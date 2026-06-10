<script lang="ts">
	import './layout.css';
	import Titlebar from '$lib/components/Titlebar.svelte';
	import { native } from '$lib/native.svelte';

	let { children } = $props();

	$effect(() => {
		document.documentElement.style.setProperty('--accent', native.accentColor);
	});
</script>

<div class="flex h-dvh flex-col overflow-hidden">
	<Titlebar />
	<div class="min-h-0 flex-1 overflow-y-auto [scrollbar-gutter:stable]">
		{@render children()}
	</div>
</div>
