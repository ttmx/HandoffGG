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

<section class="mb-5 flex items-center" aria-label="Headset status">
	<div
		class="flex min-h-10.5 w-[min(100%,544px)] items-center gap-3 rounded-full border border-(--border) bg-(--surface) py-1.5 pr-3.5 pl-2.5"
	>
		<div
			class="relative flex h-6 w-10.5 shrink-0 items-stretch rounded-md border border-(--border-strong) p-0.75"
			role="meter"
			aria-label="Battery"
			aria-valuemin="0"
			aria-valuemax="100"
			aria-valuenow={batteryFill}
			aria-valuetext={batteryLabel}
		>
			<span
				class="absolute top-1/2 -right-1.25 h-3 w-1 -translate-y-1/2 rounded-r-[3px] bg-(--border-strong)"
				aria-hidden="true"
			></span>
			<span class="h-full rounded-[3px] bg-(--ok)" style={`width: ${batteryFill}%`}
			></span>
			<strong
				class="absolute top-1/2 left-1/2 z-1 -translate-x-1/2 -translate-y-1/2 text-[11px] leading-none font-bold text-(--accent-contrast) [text-shadow:0_1px_2px_rgba(0,0,0,0.35)] pb-px"
			>
				{batteryLabel}
			</strong>
		</div>
		<div class="grid min-w-0 w-full grid-cols-[20px_minmax(180px,1fr)_20px] items-center gap-2.25 text-(--text-soft)">
			<span class="inline-flex h-5 w-5 items-center justify-center text-(--text-muted)" title="Game">
				<Gamepad2 size={16} strokeWidth={2.2} aria-hidden="true" />
				<span class="sr-only">Game</span>
			</span>
			<div
				class={`relative h-5.5 ${hasChatMix ? '' : 'opacity-45'}`}
				role="meter"
				aria-label="ChatMix balance"
				aria-valuemin="0"
				aria-valuemax="100"
				aria-valuenow={chatMixPosition}
				aria-valuetext={chatMixLabel}
			>
				<span
					class="absolute inset-x-0 top-1.75 block h-2 rounded-full bg-[linear-gradient(90deg,var(--accent),var(--border-strong)_50%,var(--ok))]"
				></span>
				<span
					class="absolute top-0.75 left-1/2 h-4 w-0.5 -translate-x-1/2 rounded-full border border-(--border-strong) bg-(--surface)"
				></span>
				<span
					class="absolute top-px h-5 w-5 -translate-x-1/2 rounded-full border-2 border-(--text-soft) bg-(--surface) shadow-[0_1px_3px_rgba(0,0,0,0.16)]"
					style={`left: ${chatMixPosition}%`}
				></span>
			</div>
			<span class="inline-flex h-5 w-5 items-center justify-center text-(--text-muted)" title="Chat">
				<MessageCircle size={16} strokeWidth={2.2} aria-hidden="true" />
				<span class="sr-only">Chat</span>
			</span>
		</div>
	</div>
</section>
