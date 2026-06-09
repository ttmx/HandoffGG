<script lang="ts">
	import type { PresenceSnapshot } from '$lib/native.svelte';

	let { presence }: { presence: PresenceSnapshot | null } = $props();

</script>

<section class="panel">
	<h2>HID Status</h2>
	<dl>
		<dt>Raw response</dt>
		<dd>{presence?.rawResponse ?? 'none'}</dd>
		<dt>Device path</dt>
		<dd>{presence?.devicePath ?? 'not found'}</dd>
		<dt>Error</dt>
		<dd>{presence?.error ?? 'none'}</dd>
	</dl>
</section>
