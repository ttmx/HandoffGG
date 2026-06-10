<script lang="ts">
	import type { DiagnosticEvent } from '$lib/native.svelte';

	let { diagnostics }: { diagnostics: DiagnosticEvent[] } = $props();
</script>

<section class="mb-6">
	<h2 class="mb-3 text-sm font-semibold">Diagnostics</h2>
	<div class="grid max-h-[260px] gap-2 overflow-auto">
		{#each diagnostics as event, index (index)}
			<article class="grid grid-cols-[90px_48px_1fr] items-baseline gap-2.5 border-b border-[var(--border)] py-2">
				<time class="text-[13px] text-[var(--text-muted)]">
					{new Date(Number(event.timestampMs)).toLocaleTimeString()}
				</time>
				<strong class="text-[13px] text-[var(--text-muted)]">{event.level}</strong>
				<span>{event.message}</span>
			</article>
		{/each}
	</div>
</section>
