<script lang="ts">
	import { api, clearToken, Unauthorized } from '$lib/api';
	import type { DishView, QueueView } from '$lib/types';
	import Thread from '$lib/components/Thread.svelte';

	let queue: QueueView | null = $state(null);
	let error: string | null = $state(null);

	async function reload() {
		try {
			queue = await api.queue();
			error = null;
		} catch (e) {
			if (e instanceof Unauthorized) {
				clearToken();
				location.reload();
				return;
			}
			error = String(e);
		}
	}

	$effect(() => {
		reload();
	});

	function verdictText(d: DishView): string {
		const v = d.verdict;
		switch (v.kind) {
			case 'idea':
				return 'idea — no recipe yet';
			case 'ready':
				return 'ready now';
			case 'lead':
				return `start now: ${v.step} → ready ${v.ready_date} ${v.ready_time}`;
			case 'shop':
				return `shop — ${v.tier_name}: ${v.items.join(', ')}`;
			case 'missing-equipment':
				return `missing equipment here: ${v.items.join(', ')}`;
		}
	}

	function verdictClass(d: DishView): string {
		return { idea: 'secondary', ready: 'ready', lead: 'lead', shop: 'shop', 'missing-equipment': 'blocked' }[
			d.verdict.kind
		];
	}
</script>

{#if error}
	<article><p>⚠ {error}</p></article>
{:else if queue}
	<hgroup>
		<h2>Queue — {queue.location}</h2>
		<p>cooking for {queue.headcount}</p>
	</hgroup>

	{#if queue.entries.length === 0}
		<p><em>Queue is empty — ask below.</em></p>
	{/if}
	{#each queue.entries as entry (entry.id)}
		<article>
			{#each entry.dishes as dish, i (i)}
				<div>
					{#if dish.recipe}
						<a href={`/page/recipes/${dish.recipe}`}><strong>{dish.title}</strong></a>
						<small>[{dish.effort}]</small>
					{:else}
						<strong>{dish.title}</strong>
					{/if}
					<span class={`verdict ${verdictClass(dish)}`}>{verdictText(dish)}</span>
					{#if dish.unlinked > 0}
						<small>· {dish.unlinked} unlinked ingredient{dish.unlinked === 1 ? '' : 's'}</small>
					{/if}
				</div>
			{/each}
			<footer>
				<small>
					{#if entry.reason}why: {entry.reason} —{/if}
					added {entry.added}{entry.age_days ? `, ${entry.age_days}d on the queue` : ''}
				</small>
			</footer>
		</article>
	{/each}

	<p>
		{#if queue.coverage.dinners === 0}
			<mark>Fridge: nothing cooked — you run out of food today</mark>
		{:else}
			Fridge: {queue.coverage.dinners} dinner{queue.coverage.dinners === 1 ? '' : 's'} covered —
			you run out {queue.coverage.runs_out}
		{/if}
		{#if queue.coverage.freezer_dinners > 0}
			<small>
				— unless you defrost: +{queue.coverage.freezer_dinners} to
				{queue.coverage.runs_out_with_freezer}
			</small>
		{/if}
	</p>

	{#if queue.someday.length > 0}
		<details>
			<summary>Someday shelf ({queue.someday.length})</summary>
			<ul>
				{#each queue.someday as s (s.id)}
					<li>{s.id}: {s.titles.join(' + ')}</li>
				{/each}
			</ul>
		</details>
	{/if}

	<hr />
	<h3>Planning</h3>
	<Thread thread="planning" onExchangeDone={reload} />
{:else}
	<article aria-busy="true">Loading the queue…</article>
{/if}

<style>
	.verdict {
		margin-left: 0.5rem;
		font-size: 0.85em;
	}
	.verdict.ready {
		color: var(--pico-ins-color, #2e7d32);
	}
	.verdict.blocked {
		color: var(--pico-del-color, #b71c1c);
	}
</style>
