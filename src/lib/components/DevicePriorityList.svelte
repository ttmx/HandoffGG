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
    import { Headphones } from '@lucide/svelte';

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

<div class="mb-6">
	<h2 class="mb-3 text-sm font-semibold">{title}</h2>

	<p class="mt-1 mb-2 text-[13px] font-semibold text-(--text-soft)">
		Priority
	</p>
	<section
		class="grid min-h-13 auto-rows-max content-start gap-1.5 rounded-lg border border-dashed border-(--border-strong) p-1.5"
		use:dndzone={{ items: priorityRows, flipDurationMs: FLIP_MS, dropTargetStyle: {} }}
		onconsider={(e) => consider('priority', e.detail.items as DeviceRow[])}
		onfinalize={(e) => finalize('priority', e.detail.items as DeviceRow[])}
	>
		{#each priorityRows as row (row.id)}
			{@const available = isRowAvailable(row, connected)}
			<article
				class={`flex cursor-grab items-center gap-2.25 rounded-md border bg-(--surface) px-2.5 py-1.5 active:cursor-grabbing ${
					row.id === activeId
						? 'border-(--accent) shadow-[inset_3px_0_0_var(--accent)]'
						: 'border-(--border)'
				} ${available ? '' : 'opacity-55'}`}
				animate:flip={{ duration: FLIP_MS }}
			>
				<span class="text-[15px] leading-none text-(--text-muted)" aria-hidden="true">⠿</span>
				{#if row.isPresenceTracked}
					<span
						class={`text-[15px] ${connected ? 'opacity-100' : 'opacity-45 grayscale'}`}
						title={stateLabel(row)}
					>
						<Headphones size={14} strokeWidth={2} />
					</span>
				{/if}
				<span
					class={`flex-1 overflow-hidden text-ellipsis whitespace-nowrap ${
						available ? '' : 'line-through decoration-(--text-muted)'
					}`}
				>
					{row.name}
				</span>
				<span
					class={`shrink-0 rounded-full px-2 py-0.5 text-[11px] ${
						available
							? 'bg-[color-mix(in_srgb,var(--ok)_18%,transparent)] text-(--ok)'
							: 'bg-(--hover) text-(--text-muted)'
					}`}
				>
					{stateLabel(row)}
				</span>
			</article>
		{/each}
		{#if priorityRows.length === 0}
			<p class="m-0 px-1.5 py-2 text-center text-[13px] text-(--text-muted)">
				Drag devices here to prioritise them.
			</p>
		{/if}
	</section>

	<p class="mt-4.5 mb-2 text-[13px] font-medium text-(--text-muted)">
		Excluded
	</p>
	<section
		class="grid min-h-13 auto-rows-max content-start gap-1.5 rounded-lg border border-dashed border-(--border-strong) bg-(--hover) p-1.5"
		use:dndzone={{ items: excludedRows, flipDurationMs: FLIP_MS, dropTargetStyle: {} }}
		onconsider={(e) => consider('excluded', e.detail.items as DeviceRow[])}
		onfinalize={(e) => finalize('excluded', e.detail.items as DeviceRow[])}
	>
		{#each excludedRows as row (row.id)}
			<article
				class="flex cursor-grab items-center gap-2.25 rounded-md border border-(--border) bg-transparent px-2.5 py-1.5 active:cursor-grabbing"
				animate:flip={{ duration: FLIP_MS }}
			>
				<span class="text-[15px] leading-none text-(--text-muted)" aria-hidden="true">⠿</span>
				{#if row.isPresenceTracked}
					<span
						class={`text-[15px] ${connected ? 'opacity-100' : 'opacity-45 grayscale'}`}
						title={stateLabel(row)}
					>
						🎧
					</span>
				{/if}
				<span class="flex-1 overflow-hidden text-ellipsis whitespace-nowrap">{row.name}</span>
				<span class="shrink-0 rounded-full bg-(--hover) px-2 py-0.5 text-[11px] text-(--text-muted)">
					{stateLabel(row)}
				</span>
			</article>
		{/each}
		{#if excludedRows.length === 0}
			<p class="m-0 px-1.5 py-2 text-center text-[13px] text-(--text-muted)">
				Drag devices here to ignore them.
			</p>
		{/if}
	</section>
</div>
