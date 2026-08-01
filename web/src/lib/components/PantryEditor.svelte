<script lang="ts">
	import { api } from '$lib/api';
	import type { LocationView } from '$lib/types';

	let {
		location,
		version,
		onChanged
	}: { location: string; version: number; onChanged: () => void } = $props();

	let view: LocationView | null = $state(null);
	let error: string | null = $state(null);
	let newItem = $state('');

	async function load() {
		view = await api.location();
		error = null;
	}

	// A tap reports upward; the page bumps `version` and everything —
	// this editor included — refreshes in place. One path, no remount.
	async function tap(action: string, body: Record<string, unknown>) {
		error = null;
		try {
			await api.edit(action, { ...body, location });
			onChanged();
		} catch (e) {
			error = String(e);
		}
	}

	function addItem(e: SubmitEvent) {
		e.preventDefault();
		const item = newItem.trim().toLowerCase().replace(/\s+/g, '-');
		if (!item) return;
		newItem = '';
		tap('pantry-set', { item, presence: 'have' });
	}

	$effect(() => {
		void version;
		load().catch((e) => (error = String(e)));
	});

	// $derived.by: a plain $derived expression sits at a point where TS
	// has narrowed `view` to its initializer (null); the closure sees the
	// declared type.
	let items = $derived.by(() => (view ? Object.values(view.view.pantry) : []));
	let tiers = $derived.by(() => view?.view.tiers ?? []);
	// Editing follows the active location for now; a foreign location's
	// page stays read-only until the M7 location selector.
	let editable = $derived.by(() => view?.location === location);
</script>

{#if error}
	<p>⚠ {error}</p>
{/if}
{#if view && editable}
	<ul class="items">
		{#each items as item (item.slug)}
			<li>
				<span>{item.name}</span>
				<span class="controls">
					<select
						aria-label={`presence of ${item.name}`}
						value={item.presence}
						onchange={(e) => tap('pantry-set', { item: item.slug, presence: e.currentTarget.value })}
					>
						<option value="have">have</option>
						<option value="low">low</option>
						<option value="out">out</option>
					</select>
					<select
						aria-label={`tier of ${item.name}`}
						value={item.tier ?? ''}
						onchange={(e) => tap('pantry-set', { item: item.slug, tier: e.currentTarget.value })}
					>
						<option value="" disabled>tier…</option>
						{#each tiers as tier (tier.id)}
							<option value={tier.id}>{tier.name}</option>
						{/each}
					</select>
					<button
						class="secondary outline"
						title="Remove entirely (usually you want presence: out)"
						onclick={() => tap('pantry-remove', { item: item.slug })}>✕</button
					>
				</span>
			</li>
		{/each}
	</ul>
	<form onsubmit={addItem} role="group">
		<input placeholder="New item (e.g. chicken thighs)" bind:value={newItem} />
		<button type="submit" disabled={!newItem.trim()}>Add</button>
	</form>
{/if}

<style>
	/* Rows wrap instead of overflowing: a long name pushes the controls
	   onto their own line, never off the screen. */
	ul.items {
		list-style: none;
		padding: 0;
	}
	li {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: 0.2rem 0.6rem;
		padding: 0.25rem 0;
		border-bottom: 1px solid var(--pico-muted-border-color);
	}
	.controls {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		align-items: center;
		gap: 0.4rem;
		margin-left: auto;
	}
	.controls select,
	.controls button {
		margin-bottom: 0;
		width: auto;
	}
	/* Compact: Pico's full-height fields are dead weight in a dense list. */
	.controls select {
		padding: 0.15rem 1.75rem 0.15rem 0.5rem;
	}
	.controls button {
		padding: 0.15rem 0.5rem;
	}
</style>
