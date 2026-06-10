<script lang="ts">
	import type { DiagnosticEvent } from '$lib/native.svelte';

	let { diagnostics }: { diagnostics: DiagnosticEvent[] } = $props();

	// Events without an explicit category are treated as "general".
	const categoryOf = (event: DiagnosticEvent) => event.category || 'general';

	const FILTERS = [
		{ id: 'general', label: 'General' },
		{ id: 'chatmix', label: 'ChatMix' }
	] as const;

	// Which categories are currently shown. Both on by default; toggle ChatMix off to
	// filter the noisy wheel events out, or General off to look at ChatMix on its own.
	let visible = $state<Record<string, boolean>>({ general: true, chatmix: true });

	const filtered = $derived(diagnostics.filter((event) => visible[categoryOf(event)] ?? true));
</script>

<section class="mb-6">
	<div class="mb-3 flex items-center justify-between gap-3">
		<h2 class="text-sm font-semibold">Diagnostics</h2>
		<div class="flex gap-1.5">
			{#each FILTERS as filter (filter.id)}
				<button
					type="button"
					class={[
						'rounded-full border px-2.5 py-0.5 text-[12px] font-semibold transition-colors',
						visible[filter.id]
							? 'border-[var(--accent)] bg-[var(--accent)] text-white'
							: 'border-[var(--border)] text-[var(--text-muted)]'
					]}
					aria-pressed={visible[filter.id]}
					onclick={() => (visible[filter.id] = !visible[filter.id])}
				>
					{filter.label}
				</button>
			{/each}
		</div>
	</div>
	<div class="grid max-h-[260px] gap-2 overflow-auto">
		{#each filtered as event, index (index)}
			<article class="grid grid-cols-[90px_48px_1fr] items-baseline gap-2.5 border-b border-[var(--border)] py-2">
				<time class="text-[13px] text-[var(--text-muted)]">
					{new Date(Number(event.timestampMs)).toLocaleTimeString()}
				</time>
				<strong class="text-[13px] text-[var(--text-muted)]">{event.level}</strong>
				<span>{event.message}</span>
			</article>
		{:else}
			<p class="py-2 text-[13px] text-[var(--text-muted)]">No matching events.</p>
		{/each}
	</div>
</section>
