<script lang="ts">
	import { onMount } from 'svelte';
	import { native } from '$lib/native.svelte';
	import DevicePriorityList from '$lib/components/DevicePriorityList.svelte';
	import HeadsetStatus from '$lib/components/HeadsetStatus.svelte';
	import HidStatus from '$lib/components/HidStatus.svelte';
	import DiagnosticsLog from '$lib/components/DiagnosticsLog.svelte';

	onMount(() => {
		let cleanup: (() => void) | undefined;
		native.start().then((stop) => (cleanup = stop));
		return () => cleanup?.();
	});
</script>

<main>
	{#if native.error}
		<section class="notice error">{native.error}</section>
	{/if}

	{#if native.busy || !native.config}
		<section class="panel">Loading...</section>
	{:else}
		<HeadsetStatus presence={native.presence} />

		<section class="grid">
			<DevicePriorityList
				title="Output Devices"
				flow="render"
				bind:prefs={native.config.output.priorities}
				endpoints={native.outputEndpoints}
				connected={native.presence?.connected ?? false}
				oncommit={native.persistAndApply}
			/>
			<DevicePriorityList
				title="Input Devices"
				flow="capture"
				bind:prefs={native.config.input.priorities}
				endpoints={native.inputEndpoints}
				connected={native.presence?.connected ?? false}
				oncommit={native.persistAndApply}
			/>
		</section>

		<details class="debug">
			<summary>Debug</summary>
			<HidStatus presence={native.presence} />
			<DiagnosticsLog diagnostics={native.diagnostics} />
		</details>
	{/if}
</main>
