<script lang="ts">
	import { dndzone } from 'svelte-dnd-action';
	import { flip } from 'svelte/animate';
	import {
		mergeFlow,
		isRowAvailable,
		type DevicePref,
		type DeviceRow,
		type AudioEndpoint,
		type EndpointFlow,
	} from '$lib/native.svelte';

	let {
		title,
		flow,
		prefs = $bindable(),
		endpoints,
		connected,
		oncommit,
	}: {
		title: string;
		flow: EndpointFlow;
		prefs: DevicePref[];
		endpoints: AudioEndpoint[];
		connected: boolean;
		oncommit?: () => void;
	} = $props();

	const FLIP_MS = 160;

	let priorityRows = $state<DeviceRow[]>([]);
	let excludedRows = $state<DeviceRow[]>([]);
	let dragging = false;

	// Resync the two zones from saved prefs + live endpoints whenever they change,
	// but never while a drag is in flight (a presence event must not reorder the
	// list under the user's cursor).
	$effect(() => {
		const merged = mergeFlow(prefs, endpoints, flow);
		if (dragging) return;
		priorityRows = merged.filter((row) => !row.excluded);
		excludedRows = merged.filter((row) => row.excluded);
	});

	// The device this flow would currently route to: first available, in order.
	let activeId = $derived(priorityRows.find((row) => isRowAvailable(row, connected))?.id);

	function consider(zone: 'priority' | 'excluded', items: DeviceRow[]) {
		dragging = true;
		if (zone === 'priority') priorityRows = items;
		else excludedRows = items;
	}

	function finalize(zone: 'priority' | 'excluded', items: DeviceRow[]) {
		if (zone === 'priority') priorityRows = items;
		else excludedRows = items;
		dragging = false;
		prefs = [
			...priorityRows.map((row) => ({ id: row.id, name: row.name, excluded: false })),
			...excludedRows.map((row) => ({ id: row.id, name: row.name, excluded: true })),
		];
		oncommit?.();
	}

	function stateLabel(row: DeviceRow): string {
		if (!row.present) return 'offline';
		if (row.isPresenceTracked) return connected ? 'connected' : 'disconnected';
		return row.state;
	}
</script>

<div class="panel device-list">
	<h2>{title}</h2>

	<p class="zone-label">Priority — top device wins</p>
	<section
		class="zone"
		use:dndzone={{ items: priorityRows, flipDurationMs: FLIP_MS, dropTargetStyle: {} }}
		onconsider={(e) => consider('priority', e.detail.items as DeviceRow[])}
		onfinalize={(e) => finalize('priority', e.detail.items as DeviceRow[])}
	>
		{#each priorityRows as row (row.id)}
			{@const available = isRowAvailable(row, connected)}
			<article
				class="row"
				class:active={row.id === activeId}
				class:skipped={!available}
				animate:flip={{ duration: FLIP_MS }}
			>
				<span class="grip" aria-hidden="true">⠿</span>
				{#if row.isPresenceTracked}
					<span class="headset" class:on={connected} title={stateLabel(row)}>🎧</span>
				{/if}
				<span class="name">{row.name}</span>
				<span class="badge" class:ok={available}>{stateLabel(row)}</span>
			</article>
		{/each}
		{#if priorityRows.length === 0}
			<p class="empty">Drag devices here to prioritise them.</p>
		{/if}
	</section>

	<p class="zone-label muted">Excluded — never selected</p>
	<section
		class="zone excluded"
		use:dndzone={{ items: excludedRows, flipDurationMs: FLIP_MS, dropTargetStyle: {} }}
		onconsider={(e) => consider('excluded', e.detail.items as DeviceRow[])}
		onfinalize={(e) => finalize('excluded', e.detail.items as DeviceRow[])}
	>
		{#each excludedRows as row (row.id)}
			<article class="row excluded-row" animate:flip={{ duration: FLIP_MS }}>
				<span class="grip" aria-hidden="true">⠿</span>
				{#if row.isPresenceTracked}
					<span class="headset" class:on={connected} title={stateLabel(row)}>🎧</span>
				{/if}
				<span class="name">{row.name}</span>
				<span class="badge">{stateLabel(row)}</span>
			</article>
		{/each}
		{#if excludedRows.length === 0}
			<p class="empty">Drag devices here to ignore them.</p>
		{/if}
	</section>
</div>
