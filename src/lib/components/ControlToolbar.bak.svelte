<script lang="ts">
	import { native } from '$lib/native.svelte';
</script>

<section class="toolbar">
	{#if native.config}
		<label class="toggle">
			<input
				bind:checked={native.config.autoswitchEnabled}
				onchange={native.persistAndApply}
				type="checkbox"
			/>
			Autoswitch
		</label>
	{/if}
	{#if native.saved}<span class="saved">Saved</span>{/if}
</section>
