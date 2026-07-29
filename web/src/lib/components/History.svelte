<script lang="ts">
	import { api } from '$lib/api';
	import type { ChangeInfo } from '$lib/types';

	let { doc, onReverted }: { doc: string; onReverted?: () => void } = $props();

	let changes: ChangeInfo[] = $state([]);
	let error: string | null = $state(null);
	let busy = $state(false);

	async function reload() {
		changes = (await api.history(doc)).changes;
	}

	$effect(() => {
		void doc;
		reload().catch((e) => (error = String(e)));
	});

	async function revert(hash: string) {
		if (busy) return;
		busy = true;
		error = null;
		try {
			await api.revert(doc, hash);
			await reload();
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
	<table>
		<tbody>
			{#each [...changes].reverse() as change, i (change.hash)}
				<tr>
					<td>{when(change.time)}</td>
					<td>{change.message}</td>
					<td>
						{#if i > 0}
							<button
								class="secondary outline"
								disabled={busy}
								onclick={() => revert(change.hash)}>revert to</button
							>
						{:else}
							<small>current</small>
						{/if}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</details>
