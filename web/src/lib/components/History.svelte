<script lang="ts">
	import { api } from '$lib/api';
	import type { ChangeInfo } from '$lib/types';

	let {
		doc,
		version,
		onReverted
	}: { doc: string; version: number; onReverted?: () => void } = $props();

	let changes: ChangeInfo[] = $state([]);
	let error: string | null = $state(null);
	let busy = $state(false);

	async function reload() {
		changes = (await api.history(doc)).changes;
		error = null;
	}

	// Reloads in place on every version bump; the <details> open state and
	// the scroll position survive an edit made anywhere on the page.
	$effect(() => {
		void doc;
		void version;
		reload().catch((e) => (error = String(e)));
	});

	async function revert(hash: string) {
		if (busy) return;
		busy = true;
		error = null;
		try {
			await api.revert(doc, hash);
			onReverted?.();
		} catch (e) {
			error = String(e);
		} finally {
			busy = false;
		}
	}

	function when(time: string | null): string {
		return time ? time.replace('T', ' ').replace(/:\d\dZ$/, '') : '';
	}
</script>

<details>
	<summary>Recent changes ({changes.length})</summary>
	{#if error}<p>⚠ {error}</p>{/if}
	<ul class="changes">
		{#each [...changes].reverse() as change, i (change.hash)}
			<li>
				<small>{when(change.time)}</small>
				<span class="msg">{change.message}</span>
				{#if i > 0}
					<button class="secondary outline" disabled={busy} onclick={() => revert(change.hash)}
						>revert to</button
					>
				{:else}
					<small>current</small>
				{/if}
			</li>
		{/each}
	</ul>
</details>

<style>
	/* Rows wrap instead of overflowing: a long message takes its own line
	   on a phone rather than pushing the button off the screen. */
	ul.changes {
		list-style: none;
		padding: 0;
	}
	li {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.2rem 0.6rem;
		padding: 0.2rem 0;
	}
	.msg {
		flex: 1 1 12rem;
	}
	li button {
		margin-bottom: 0;
		width: auto;
		padding: 0.1rem 0.6rem;
	}
</style>
