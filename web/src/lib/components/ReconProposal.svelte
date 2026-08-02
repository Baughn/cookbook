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
	let failed: Record<string, string> = $state({});
	let busy = $state(false);
	let error: string | null = $state(null);

	// Each proposal starts its own tap bookkeeping; lines the pantry
	// already satisfies arrive pre-applied via `current` — done-ness is
	// derived from the pantry, not remembered, so hand edits count too.
	// Identity is the proposed lines, not the object: every apply
	// refreshes the thread and hands down a fresh object for the *same*
	// proposal, which must not wipe the failure markers mid-batch. (The
	// volatile `current` stays out of the key for the same reason.)
	let bookkeepingFor = '';
	$effect(() => {
		const key = JSON.stringify(proposal.lines.map((l) => [l.item, l.presence]));
		if (key !== bookkeepingFor) {
			bookkeepingFor = key;
			applied = {};
			failed = {};
		}
	});

	function done(line: ReconLine): boolean {
		return applied[line.item] || line.current === line.presence;
	}

	// The one place a line's pantry-set happens. Success and failure are
	// per-line state, and the return value lets a batch count outcomes —
	// a caller can't be left believing a step worked when it didn't.
	async function applyOne(line: ReconLine): Promise<boolean> {
		try {
			await api.edit('pantry-set', {
				item: line.item,
				presence: line.presence,
				...(line.name ? { name: line.name } : {}),
				...(proposal.location ? { location: proposal.location } : {})
			});
			applied[line.item] = true;
			delete failed[line.item];
			onApplied();
			return true;
		} catch (e) {
			failed[line.item] = String(e);
			return false;
		}
	}

	async function apply(line: ReconLine) {
		// The busy guard covers a double-tap: the second click lands
		// before the disabled attribute has rendered.
		if (busy) return;
		busy = true;
		error = null;
		if (!(await applyOne(line))) error = failed[line.item];
		busy = false;
	}

	async function applyAll() {
		if (busy) return;
		busy = true;
		// Cleared once for the batch, summarized once after it — never
		// per line, where a success after a failure erases the evidence.
		error = null;
		const todo = proposal.lines.filter((l) => !done(l));
		let ok = 0;
		for (const line of todo) {
			if (await applyOne(line)) ok += 1;
		}
		const failures = todo.filter((l) => failed[l.item]);
		if (failures.length > 0) {
			const names = failures.map((l) => l.name ?? l.item).join(', ');
			error = `${ok} of ${todo.length} applied; ${names} failed: ${failed[failures[0].item]}`;
		}
		busy = false;
	}

	let remaining = $derived(proposal.lines.filter((l) => !done(l)).length);
</script>

<article aria-label="recon proposal">
	{#if remaining === 0}
		<!-- Done proposals collapse; the server also stops returning them,
		     so a reload clears even this line. -->
		<p class="done">✓ All applied.</p>
	{:else}
		<header>
			<strong>Proposed from the photo</strong> — tap what's right; nothing applies on its own.
		</header>
		{#if error}
			<p>⚠ {error}</p>
		{/if}
		<ul class="lines">
			{#each proposal.lines as line (line.item)}
				<li>
					<div class="row">
						<span>{line.name ?? line.item} <code>{line.presence}</code></span>
						{#if done(line)}
							<span aria-label={`applied ${line.item}`}>✓</span>
						{:else}
							{#if failed[line.item]}
								<span aria-label={`failed ${line.item}`} title={failed[line.item]}>⚠</span>
							{/if}
							<button
								class="outline"
								disabled={busy}
								onclick={() => apply(line)}
								aria-label={`apply ${line.item} ${line.presence}`}
							>
								Apply
							</button>
						{/if}
					</div>
					<small>{line.reason}</small>
				</li>
			{/each}
		</ul>
		<footer>
			<button disabled={busy} onclick={applyAll}>Apply all ({remaining})</button>
			<small>Wrong or missing something? Say so below — your words beat the photo.</small>
		</footer>
	{/if}
</article>

<style>
	/* Stacked rows, not a table: the prose reason gets its own line, so
	   nothing has a minimum width wider than a phone. */
	ul.lines {
		list-style: none;
		padding: 0;
		margin-bottom: 0;
	}
	li {
		padding: 0.3rem 0;
	}
	li + li {
		border-top: 1px solid var(--pico-muted-border-color);
	}
	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 0.6rem;
	}
	.row button {
		margin-bottom: 0;
		padding: 0.1rem 0.6rem;
		width: auto;
	}
	footer {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.5rem 1rem;
	}
	footer button {
		margin-bottom: 0;
		width: auto;
	}
	.done {
		margin-bottom: 0;
	}
</style>
