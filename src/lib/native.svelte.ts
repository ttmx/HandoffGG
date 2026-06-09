import { invoke as apiInvoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

export type EndpointFlow = 'render' | 'capture';
export type EndpointState = 'active' | 'disabled' | 'notPresent' | 'unplugged' | 'unknown';

export type AudioEndpoint = {
	id: string;
	name: string;
	flow: EndpointFlow;
	state: EndpointState;
	isPresenceTracked: boolean;
	isDefaultConsole: boolean;
	isDefaultMultimedia: boolean;
	isDefaultCommunications: boolean;
};

/** One entry in a flow's priority list. */
export type DevicePref = {
	id: string;
	name: string;
	excluded: boolean;
};

export type FlowConfig = {
	/** Ordered priority list: index 0 is the highest priority. */
	priorities: DevicePref[];
};

export type AppConfig = {
	autoswitchEnabled: boolean;
	output: FlowConfig;
	input: FlowConfig;
};

/** A priority-list row enriched with the matching live endpoint, for rendering. */
export type DeviceRow = DevicePref & {
	flow: EndpointFlow;
	state: EndpointState;
	isPresenceTracked: boolean;
	/** False when no live endpoint currently matches this saved pref (offline). */
	present: boolean;
};

/**
 * Build the display rows for one flow: keep the saved order/exclusions, enrich each
 * with its live endpoint, and append any live endpoint not yet saved (non-excluded,
 * lowest priority). Devices that are not present — removed hardware, or a saved pref
 * with no matching live endpoint — are dropped so the list only shows real devices.
 * (A SteelSeries dongle stays present while the headset is off, so it is unaffected.)
 */
export function mergeFlow(
	prefs: DevicePref[],
	endpoints: AudioEndpoint[],
	flow: EndpointFlow,
): DeviceRow[] {
	const rows: DeviceRow[] = prefs.map((pref) => {
		const endpoint = endpoints.find((candidate) => candidate.id === pref.id);
		return {
			...pref,
			name: endpoint?.name ?? pref.name,
			flow,
			state: endpoint?.state ?? 'notPresent',
			isPresenceTracked: endpoint?.isPresenceTracked ?? false,
			present: endpoint !== undefined,
		};
	});

	for (const endpoint of endpoints) {
		if (prefs.some((pref) => pref.id === endpoint.id)) continue;
		rows.push({
			id: endpoint.id,
			name: endpoint.name,
			excluded: false,
			flow,
			state: endpoint.state,
			isPresenceTracked: endpoint.isPresenceTracked,
			present: true,
		});
	}

	return rows.filter((row) => row.state !== 'notPresent');
}

/**
 * Whether a row would currently be skipped by the switcher: not Active, or a
 * presence-tracked device while the headset is disconnected.
 */
export function isRowAvailable(row: DeviceRow, headsetConnected: boolean): boolean {
	if (row.state !== 'active') return false;
	if (row.isPresenceTracked && !headsetConnected) return false;
	return true;
}

export type PresenceSnapshot = {
	connected: boolean;
	hasConnectionStatus: boolean;
	micMuted: boolean | null;
	batteryPercent: number | null;
	gameVolume: number | null;
	chatVolume: number | null;
	rawResponse: string | null;
	devicePath: string | null;
	error: string | null;
	observedAtMs: number;
};

export type DiagnosticEvent = {
	timestampMs: number;
	level: string;
	message: string;
};

const STATE_CHANGED_EVENT = 'autoswapper://state-changed';

/** Used when the platform exposes no system accent (e.g. non-GNOME Linux). */
const FALLBACK_ACCENT = '#3584e4';

/**
 * Reactive bridge to the native (Tauri) backend. Holds the latest values pulled
 * from the backend as reactive state, and keeps them up to date by listening for
 * the `state-changed` event the backend emits on every meaningful change.
 *
 * Use a single shared instance (`native`) so every component reads the same state.
 */
class NativeBridge {
	config = $state<AppConfig | null>(null);
	endpoints = $state<AudioEndpoint[]>([]);
	presence = $state<PresenceSnapshot | null>(null);
	diagnostics = $state<DiagnosticEvent[]>([]);
	busy = $state(true);
	error = $state('');
	saved = $state(false);
	accentColor = $state(FALLBACK_ACCENT);

	outputEndpoints = $derived(this.endpoints.filter((endpoint) => endpoint.flow === 'render'));
	inputEndpoints = $derived(this.endpoints.filter((endpoint) => endpoint.flow === 'capture'));

	#savedTimer: ReturnType<typeof setTimeout> | undefined;

	/**
	 * Start receiving live updates and load the initial backend state. The listener
	 * is registered before the first fetch so no event emitted during startup is
	 * missed. Returns a cleanup function that stops the listener — call it on
	 * component teardown.
	 */
	start = async (): Promise<() => void> => {
		const unlisten = await listen(STATE_CHANGED_EVENT, this.refreshLiveState);
		// The system accent can change while the app runs; the user has to leave the
		// window to change it, so refetching on focus keeps us in sync without polling.
		const unfocus = await getCurrentWindow().onFocusChanged(({ payload: focused }) => {
			if (focused) this.loadAccent();
		});
		await Promise.all([this.refreshAll(), this.loadAccent()]);
		return () => {
			unlisten();
			unfocus();
		};
	};

	loadAccent = async (): Promise<void> => {
		try {
			const accent = await apiInvoke<string | null>('get_accent_color');
			this.accentColor = accent ?? FALLBACK_ACCENT;
		} catch {
			this.accentColor = FALLBACK_ACCENT;
		}
	};

	refreshAll = async (): Promise<void> => {
		this.busy = true;
		this.error = '';
		try {
			const [nextConfig, nextEndpoints, nextPresence, nextDiagnostics] = await Promise.all([
				apiInvoke<AppConfig>('get_config'),
				apiInvoke<AudioEndpoint[]>('list_endpoints'),
				apiInvoke<PresenceSnapshot>('get_presence'),
				apiInvoke<DiagnosticEvent[]>('get_diagnostics'),
			]);
			this.config = nextConfig;
			this.endpoints = nextEndpoints;
			this.presence = nextPresence;
			this.diagnostics = nextDiagnostics.reverse();
		} catch (err) {
			this.error = String(err);
		} finally {
			this.busy = false;
		}
	};

	refreshLiveState = async (): Promise<void> => {
		try {
			const [nextPresence, nextDiagnostics] = await Promise.all([
				apiInvoke<PresenceSnapshot>('get_presence'),
				apiInvoke<DiagnosticEvent[]>('get_diagnostics'),
			]);
			this.presence = nextPresence;
			this.diagnostics = nextDiagnostics.reverse();
		} catch (err) {
			this.error = String(err);
		}
	};

	save = async (): Promise<void> => {
		if (!this.config) return;
		this.saved = false;
		this.error = '';
		try {
			this.config = await apiInvoke<AppConfig>('save_config', { newConfig: this.config });
			this.saved = true;
			clearTimeout(this.#savedTimer);
			this.#savedTimer = setTimeout(() => (this.saved = false), 1600);
		} catch (err) {
			this.error = String(err);
		}
	};

	/** Persist the current config, then immediately re-apply the priority rules. */
	persistAndApply = async (): Promise<void> => {
		await this.save();
		await this.applyNow();
	};

	applyNow = async (): Promise<void> => {
		this.error = '';
		try {
			await apiInvoke('apply_now');
			await this.refreshLiveState();
		} catch (err) {
			this.error = String(err);
		}
	};
}

export const native = new NativeBridge();
