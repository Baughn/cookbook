<script lang="ts">
	// A recon proposal as tappable lines: each Apply is exactly one
	// pantry-set tap; nothing happens without one. Corrections that a tap
	// can't express are words in the thread below — they outrank the photo.
	import { api } from '$lib/api';
	import type { ReconLine, ReconProposal } from '$lib/types';

	let {
		proposal,
		onApplied
	}: { proposal: ReconProposal; onApplied: () => void } = $props();

	let applied: Record<string, boolean> = $state({});
	let busy = $state(false);
	let error: string | null = $state(null);

	async function apply(line: ReconLine) {
		error = null;
		try {
			await api.edit('pantry-set', {
				item: line.item,
				presence: line.presence,
				...(line.name ? { name: line.name } : {}),
				...(proposal.location ? { location: proposal.location } : {})
			});
			applied[line.item] = true;
			onApplied();
		} catch (e) {
			error = String(e);
		}
	}

	async function applyAll() {
		busy = true;
		for (const line of proposal.lines) {
			if (!applied[line.item]) await apply(line);
		}
		busy = false;
	}

	let remaining = $derived(proposal.lines.filter((l) => !applied[l.item]).length);
</script>

<article aria-label="recon proposal">
	<header>
		<strong>Proposed from the photo</strong> — tap what's right; nothing applies on its own.
	</header>
	{#if error}
		<p>⚠ {error}</p>
	{/if}
	<table>
		<tbody>
			{#each proposal.lines as line (line.item)}
				<tr>
					<td>{line.name ?? line.item}</td>
					<td><code>{line.presence}</code></td>
					<td><small>{line.reason}</small></td>
					<td>
						{#if applied[line.item]}
							<span aria-label={`applied ${line.item}`}>✓</span>
						{:else}
							<button
								class="outline"
								disabled={busy}
								onclick={() => apply(line)}
								aria-label={`apply ${line.item} ${line.presence}`}
							>
								Apply
							</button>
						{/if}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
	<footer>
		{#if remaining > 0}
			<button disabled={busy} onclick={applyAll}>Apply all ({remaining})</button>
		{:else}
			<small>All applied.</small>
		{/if}
		<small>Wrong or missing something? Say so below — your words beat the photo.</small>
	</footer>
</article>

<style>
	td {
		vertical-align: middle;
	}
	td button {
		margin-bottom: 0;
		padding: 0.1rem 0.6rem;
		width: auto;
	}
	footer {
		display: flex;
		align-items: center;
		gap: 1rem;
	}
	footer button {
		margin-bottom: 0;
		width: auto;
	}
</style>
