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

<section class="app-routing panel">
	<div class="section-heading">
		<h2>App Routing</h2>
		<p>Assign running apps to the Game or Chat side of ChatMix.</p>
	</div>

	{#if sessions.length === 0}
		<p class="empty">No active app audio sessions.</p>
	{:else}
		<div class="app-session-list">
			{#each sessions as session (session.id)}
				<article class="app-session">
					<div class="app-session-main">
						<strong>{session.displayName}</strong>
						<span>{appSubtitle(session)}</span>
					</div>
					<div class="app-session-meta">
						<div class="segmented" role="group" aria-label={`${session.displayName} ChatMix route`}>
							<button
								class:active={session.route === 'game'}
								type="button"
								onclick={() => onroute(session.appId, 'game', session.displayName)}
							>
								Game
							</button>
							<button
								class:active={session.route === 'chat'}
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
