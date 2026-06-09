<script lang="ts">
	import { onMount } from 'svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { native } from '$lib/native.svelte';

	const appWindow = getCurrentWindow();
	let maximized = $state(false);

	// On Windows the OS ships icon fonts ("Segoe Fluent Icons" on 11, "Segoe MDL2
	// Assets" on 10) that contain the real window-chrome glyphs. Using them makes
	// the controls match the system exactly — the same trick VS Code/Chromium use.
	// Other platforms have no such standard, so we draw the icons ourselves.
	const isWindows = typeof navigator !== 'undefined' && navigator.userAgent.includes('Windows');

	// Segoe Fluent Icons / Segoe MDL2 Assets glyph codepoints.
	const glyph = {
		minimize: String.fromCodePoint(0xe921),
		maximize: String.fromCodePoint(0xe922),
		restore: String.fromCodePoint(0xe923),
		close: String.fromCodePoint(0xe8bb),
	};

	onMount(() => {
		let unlisten: (() => void) | undefined;
		appWindow.isMaximized().then((value) => (maximized = value));
		appWindow
			.onResized(() => appWindow.isMaximized().then((value) => (maximized = value)))
			.then((stop) => (unlisten = stop));
		return () => unlisten?.();
	});
</script>

<div class="titlebar" data-tauri-drag-region>
	<span class="titlebar-title" data-tauri-drag-region>Autoswapper</span>
	<div class="titlebar-status">
		{#if native.config}
			<label class="switch-control" title="Autoswitch">
				<input
					bind:checked={native.config.autoswitchEnabled}
					onchange={native.persistAndApply}
					type="checkbox"
				/>
				<span class="switch-track" aria-hidden="true">
					<span class="switch-thumb"></span>
				</span>
			</label>
		{/if}
	</div>
	<div class="titlebar-controls">
		<button class="titlebar-button" aria-label="Minimize" onclick={() => appWindow.minimize()}>
			{#if isWindows}
				<span class="chrome-glyph">{glyph.minimize}</span>
			{:else}
				<svg viewBox="0 0 10 10" aria-hidden="true"><rect x="0" y="4.5" width="10" height="1" /></svg>
			{/if}
		</button>
		<button
			class="titlebar-button"
			aria-label={maximized ? 'Restore' : 'Maximize'}
			onclick={() => appWindow.toggleMaximize()}
		>
			{#if isWindows}
				<span class="chrome-glyph">{maximized ? glyph.restore : glyph.maximize}</span>
			{:else if maximized}
				<svg viewBox="0 0 10 10" aria-hidden="true">
					<rect x="0" y="2" width="6" height="6" fill="none" stroke="currentColor" stroke-width="1" />
					<path d="M2.5 2 V0.5 H9.5 V7.5 H8" fill="none" stroke="currentColor" stroke-width="1" />
				</svg>
			{:else}
				<svg viewBox="0 0 10 10" aria-hidden="true">
					<rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" stroke-width="1" />
				</svg>
			{/if}
		</button>
		<button class="titlebar-button close" aria-label="Close" onclick={() => appWindow.close()}>
			{#if isWindows}
				<span class="chrome-glyph">{glyph.close}</span>
			{:else}
				<svg viewBox="0 0 10 10" aria-hidden="true">
					<path d="M0 0 L10 10 M10 0 L0 10" stroke="currentColor" stroke-width="1" />
				</svg>
			{/if}
		</button>
	</div>
</div>
