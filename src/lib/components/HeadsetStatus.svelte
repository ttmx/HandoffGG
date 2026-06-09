<script lang="ts">
	import { Gamepad2, MessageCircle } from '@lucide/svelte';
	import type { PresenceSnapshot } from '$lib/native.svelte';

	let { presence }: { presence: PresenceSnapshot | null } = $props();

	let batteryLabel = $derived(
		presence?.batteryPercent === null || presence?.batteryPercent === undefined
			? 'Unknown'
			: `${presence.batteryPercent}`
	);
	let batteryFill = $derived(
		presence?.batteryPercent === null || presence?.batteryPercent === undefined
			? 0
			: Math.max(0, Math.min(100, presence.batteryPercent))
	);
	let game = $derived(presence?.gameVolume ?? null);
	let chat = $derived(presence?.chatVolume ?? null);
	let hasChatMix = $derived(game !== null && chat !== null && game + chat > 0);
	let chatMixPosition = $derived.by(() => {
		if (game === null || chat === null || game + chat <= 0) return 50;
		return Math.round((chat / (game + chat)) * 100);
	});
	let chatMixLabel = $derived(hasChatMix ? `Game ${game} / Chat ${chat}` : 'Unknown');
</script>

<section class="headset-status" aria-label="Headset status">
	<div class="headset-pill">
		<div class="battery-meter" role="meter" aria-label="Battery" aria-valuemin="0" aria-valuemax="100" aria-valuenow={batteryFill} aria-valuetext={batteryLabel}>
			<span style={`height: ${batteryFill}%`}></span>
			<strong>{batteryLabel}</strong>
		</div>
		<div class="chatmix-row">
			<span class="chatmix-end" title="Game">
				<Gamepad2 size={16} strokeWidth={2.2} aria-hidden="true" />
				<span class="sr-only">Game</span>
			</span>
			<div
				class:unknown={!hasChatMix}
				class="chatmix-slider"
				role="meter"
				aria-label="ChatMix balance"
				aria-valuemin="0"
				aria-valuemax="100"
				aria-valuenow={chatMixPosition}
				aria-valuetext={chatMixLabel}
			>
				<span class="track"></span>
				<span class="center"></span>
				<span class="thumb" style={`left: ${chatMixPosition}%`}></span>
			</div>
			<span class="chatmix-end" title="Chat">
				<MessageCircle size={16} strokeWidth={2.2} aria-hidden="true" />
				<span class="sr-only">Chat</span>
			</span>
		</div>
	</div>
</section>
