<script lang="ts">
	import { onMount } from 'svelte';
	import { native } from '$lib/native.svelte';
	import AppRouting from '$lib/components/AppRouting.svelte';
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

<main class="mx-auto max-w-[1040px] p-7 max-[760px]:p-[18px]">
	{#if native.error}
		<section class="mb-[18px] rounded-lg border border-[var(--danger-border)] bg-[var(--danger-bg)] px-3.5 py-3 text-[var(--danger-text)]">
			{native.error}
		</section>
	{/if}

	{#if native.busy || !native.config}
		<section class="mb-6">Loading...</section>
	{:else}
		<HeadsetStatus presence={native.presence} />

		<section class="grid grid-cols-2 gap-[18px] max-[760px]:block">
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

		<AppRouting sessions={native.audioSessions} onroute={native.setChatMixRoute} />

		<details class="group">
			<summary
				class="flex cursor-pointer list-none items-center gap-1.5 py-1.5 text-[13px] font-semibold text-[var(--text-muted)] select-none [&::-webkit-details-marker]:hidden"
			>
				<span class="inline-block transition-transform group-open:rotate-90">▸</span>
				Debug
			</summary>
			<HidStatus presence={native.presence} />
			<DiagnosticsLog diagnostics={native.diagnostics} />
		</details>
	{/if}
</main>
