<script lang="ts">
	import { onMount, tick } from 'svelte';
	import './layout.css';
	import Titlebar from '$lib/components/Titlebar.svelte';
	import { native } from '$lib/native.svelte';

	let { children } = $props();
	let scrollPane: HTMLDivElement;
	let contentPane: HTMLDivElement;
	let visible = $state(false);
	let dragging = $state(false);
	let hasScrollbar = $state(false);
	let thumbTop = $state(0);
	let thumbHeight = $state(36);
	let hideTimer: ReturnType<typeof setTimeout> | undefined;
	let dragStartY = 0;
	let dragStartScrollTop = 0;
	const scrollbarTrackInset = 4;

	$effect(() => {
		document.documentElement.style.setProperty('--accent', native.accentColor);
	});

	function updateScrollbar() {
		if (!scrollPane) return;

		const { clientHeight, scrollHeight, scrollTop } = scrollPane;
		hasScrollbar = scrollHeight > clientHeight + 1;
		if (!hasScrollbar) {
			thumbTop = 0;
			thumbHeight = 0;
			return;
		}

		const trackHeight = Math.max(0, clientHeight - scrollbarTrackInset * 2);
		thumbHeight = Math.max(36, Math.round((clientHeight / scrollHeight) * trackHeight));
		const maxScrollTop = scrollHeight - clientHeight;
		const maxThumbTop = trackHeight - thumbHeight;
		thumbTop = maxScrollTop <= 0 ? 0 : Math.round((scrollTop / maxScrollTop) * maxThumbTop);
	}

	function showTemporarily() {
		visible = true;
		clearTimeout(hideTimer);
		if (!dragging) {
			hideTimer = setTimeout(() => {
				visible = false;
			}, 900);
		}
	}

	function handleScroll() {
		updateScrollbar();
		showTemporarily();
	}

	function handleTrackPointerDown(event: PointerEvent) {
		if (!scrollPane || !hasScrollbar || event.button !== 0) return;
		const target = event.target as HTMLElement;
		if (target.dataset.scrollbarThumb === 'true') return;

		const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
		const y = event.clientY - rect.top;
		const direction = y < thumbTop ? -1 : 1;
		scrollPane.scrollBy({ top: direction * scrollPane.clientHeight * 0.82, behavior: 'smooth' });
		showTemporarily();
	}

	function handleThumbPointerDown(event: PointerEvent) {
		if (!scrollPane || !hasScrollbar || event.button !== 0) return;
		event.preventDefault();
		event.stopPropagation();
		dragging = true;
		visible = true;
		clearTimeout(hideTimer);
		dragStartY = event.clientY;
		dragStartScrollTop = scrollPane.scrollTop;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
	}

	function handleThumbPointerMove(event: PointerEvent) {
		if (!dragging || !scrollPane) return;
		const trackHeight = Math.max(0, scrollPane.clientHeight - scrollbarTrackInset * 2);
		const maxThumbTop = trackHeight - thumbHeight;
		const maxScrollTop = scrollPane.scrollHeight - scrollPane.clientHeight;
		if (maxThumbTop <= 0 || maxScrollTop <= 0) return;

		const deltaY = event.clientY - dragStartY;
		scrollPane.scrollTop = dragStartScrollTop + (deltaY / maxThumbTop) * maxScrollTop;
		updateScrollbar();
	}

	function handleThumbPointerUp(event: PointerEvent) {
		if (!dragging) return;
		dragging = false;
		(event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
		showTemporarily();
	}

	onMount(() => {
		let paneObserver: ResizeObserver | undefined;
		let contentObserver: ResizeObserver | undefined;

		tick().then(() => {
			updateScrollbar();
			paneObserver = new ResizeObserver(updateScrollbar);
			contentObserver = new ResizeObserver(updateScrollbar);
			if (scrollPane) paneObserver.observe(scrollPane);
			if (contentPane) contentObserver.observe(contentPane);
		});

		return () => {
			clearTimeout(hideTimer);
			paneObserver?.disconnect();
			contentObserver?.disconnect();
		};
	});
</script>

<div class="flex h-dvh select-none flex-col overflow-hidden">
	<Titlebar />
	<div class="group/scroll relative min-h-0 flex-1">
		<div
			bind:this={scrollPane}
			class="h-full overflow-y-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
			onscroll={handleScroll}
		>
			<div bind:this={contentPane}>
				{@render children()}
			</div>
		</div>
		{#if hasScrollbar}
			<div
				class={`group/bar absolute inset-y-1 right-0 z-50 w-4 rounded-full transition-opacity duration-150 group-hover/scroll:opacity-100 ${
					visible || dragging ? 'opacity-100' : 'opacity-0'
				}`}
				onpointerdown={handleTrackPointerDown}
				role="presentation"
			>
				<button
					type="button"
					class={`absolute left-1/2 min-h-0 cursor-default rounded-full border-0 bg-[color-mix(in_srgb,var(--text-muted)_42%,transparent)] p-0 opacity-80 transition-[width,background-color,opacity] duration-150 group-hover/bar:w-2.5 group-hover/bar:bg-[color-mix(in_srgb,var(--text-muted)_72%,transparent)] group-hover/bar:opacity-100 focus-visible:w-2.5 focus-visible:bg-[color-mix(in_srgb,var(--text-muted)_72%,transparent)] focus-visible:opacity-100 focus-visible:outline-none ${
						dragging ? 'w-2.5 bg-[color-mix(in_srgb,var(--text-muted)_72%,transparent)] opacity-100' : 'w-0.5'
					}`}
					data-scrollbar-thumb="true"
					aria-label="Scroll page"
					style={`height: ${thumbHeight}px; transform: translate(-50%, ${thumbTop}px);`}
					onpointerdown={handleThumbPointerDown}
					onpointermove={handleThumbPointerMove}
					onpointerup={handleThumbPointerUp}
					onpointercancel={handleThumbPointerUp}
				></button>
			</div>
		{/if}
	</div>
</div>
