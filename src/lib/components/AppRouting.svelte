<script lang="ts">
	import type { AudioSession, ChatMixRoute } from '$lib/native.svelte';

	let {
		sessions,
		onroute,
	}: {
		sessions: AudioSession[];
		onroute: (appId: string, route: ChatMixRoute, displayName: string) => Promise<void>;
	} = $props();

	function appSubtitle(session: AudioSession): string {
		if (session.executablePath) return session.executablePath;
		return `Process ${session.processId}`;
	}
</script>

<section class="mt-5.5 mb-6">
	<div class="mb-3">
		<h2 class="mb-3 text-sm font-semibold">App Routing</h2>
		<p class="mt-1 text-xs text-(--text-muted)">Assign running apps to the Game or Chat side of ChatMix.</p>
	</div>

	{#if sessions.length === 0}
		<p class="m-0 px-1.5 py-2 text-center text-[13px] text-(--text-muted)">
			No active app audio sessions.
		</p>
	{:else}
		<div class="overflow-hidden rounded-lg border border-(--border)">
			{#each sessions as session (session.id)}
				<article
					class="grid grid-cols-[minmax(0,1fr)_max-content] items-center gap-3.5 border-b border-(--border) bg-(--surface) px-2.5 py-2.25 last:border-b-0"
				>
					<div class="grid min-w-0 gap-0.75">
						<strong class="overflow-hidden text-ellipsis whitespace-nowrap">{session.displayName}</strong>
						<span class="overflow-hidden text-ellipsis whitespace-nowrap text-xs text-(--text-muted)">
							{appSubtitle(session)}
						</span>
					</div>
					<div class="flex items-center gap-2">
						<div
							class="inline-flex rounded-[7px] bg-(--hover) p-0.5"
							role="group"
							aria-label={`${session.displayName} ChatMix route`}
						>
							<button
								class={`min-h-6.5 rounded-[5px] border-0 px-2.5 hover:border-0 ${
									session.route === 'game'
										? 'bg-(--accent) text-(--accent-contrast) hover:text-(--accent-contrast)'
										: 'bg-transparent text-(--text-muted) hover:text-(--text)'
								}`}
								type="button"
								onclick={() => onroute(session.appId, 'game', session.displayName)}
							>
								Game
							</button>
							<button
								class={`min-h-6.5 rounded-[5px] border-0 px-2.5 hover:border-0 ${
									session.route === 'chat'
										? 'bg-(--accent) text-(--accent-contrast) hover:text-(--accent-contrast)'
										: 'bg-transparent text-(--text-muted) hover:text-(--text)'
								}`}
								type="button"
								onclick={() => onroute(session.appId, 'chat', session.displayName)}
							>
								Chat
							</button>
						</div>
					</div>
				</article>
			{/each}
		</div>
	{/if}
</section>
