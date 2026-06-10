<script lang="ts">
	import type { PresenceSnapshot } from '$lib/native.svelte';

	let { presence }: { presence: PresenceSnapshot | null } = $props();
</script>

<section class="mb-6">
	<h2 class="mb-3 text-sm font-semibold">HID Status</h2>
	<dl class="m-0 grid grid-cols-[120px_1fr] gap-2.5">
		<dt class="text-[var(--text-muted)]">Raw response</dt>
		<dd class="m-0 break-words">{presence?.rawResponse ?? 'none'}</dd>
		<dt class="text-[var(--text-muted)]">Device path</dt>
		<dd class="m-0 break-words">{presence?.devicePath ?? 'not found'}</dd>
		<dt class="text-[var(--text-muted)]">Error</dt>
		<dd class="m-0 break-words">{presence?.error ?? 'none'}</dd>
	</dl>
</section>
