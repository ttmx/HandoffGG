<script lang="ts">
	import type { DiagnosticEvent } from '$lib/native.svelte';

	let { diagnostics }: { diagnostics: DiagnosticEvent[] } = $props();
</script>

<section class="panel">
	<h2>Diagnostics</h2>
	<div class="events">
		{#each diagnostics as event, index (index)}
			<article>
				<time>{new Date(Number(event.timestampMs)).toLocaleTimeString()}</time>
				<strong>{event.level}</strong>
				<span>{event.message}</span>
			</article>
		{/each}
	</div>
</section>
