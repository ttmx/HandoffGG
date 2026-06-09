<script lang="ts">
	import './layout.css';
	import Titlebar from '$lib/components/Titlebar.svelte';
	import { native } from '$lib/native.svelte';

	let { children } = $props();

	$effect(() => {
		document.documentElement.style.setProperty('--accent', native.accentColor);
	});
</script>

<Titlebar />
{@render children()}
