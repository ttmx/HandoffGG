<script lang="ts">
	import { Mic } from '@lucide/svelte';
	import { native, type Sidetone } from '$lib/native.svelte';

	let { sidetone }: { sidetone: Sidetone } = $props();

	const levels: { value: Sidetone; label: string }[] = [
		{ value: 'off', label: 'Off' },
		{ value: 'low', label: 'Low' },
		{ value: 'medium', label: 'Medium' },
		{ value: 'high', label: 'High' },
	];
</script>

<section class="mb-5 flex items-center gap-3" aria-label="Mic sidetone">
	<span class="flex items-center gap-1.75 text-[13px] font-semibold text-(--text-soft)">
		<Mic size={15} strokeWidth={2.2} aria-hidden="true" />
		Mic sidetone
	</span>
	<div class="inline-flex rounded-[7px] bg-(--hover) p-0.5" role="group" aria-label="Mic sidetone level">
		{#each levels as level (level.value)}
			<button
				class={`min-h-6.5 rounded-[5px] border-0 px-2.5 hover:border-0 ${
					sidetone === level.value
						? 'bg-(--accent) text-(--accent-contrast) hover:text-(--accent-contrast)'
						: 'bg-transparent text-(--text-muted) hover:text-(--text)'
				}`}
				type="button"
				aria-pressed={sidetone === level.value}
				onclick={() => native.setSidetone(level.value)}
			>
				{level.label}
			</button>
		{/each}
	</div>
</section>
