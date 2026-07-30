<script lang="ts">
	import { api } from '$lib/api';
	import type { LocationView } from '$lib/types';

	let { location, onChanged }: { location: string; onChanged: () => void } = $props();

	let view: LocationView | null = $state(null);
	let error: string | null = $state(null);
	let newItem = $state('');

	async function load() {
		view = await api.location();
	}

	async function tap(action: string, body: Record<string, unknown>) {
		error = null;
		try {
			await api.edit(action, { ...body, location });
			await load();
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
	<h3>Edit</h3>
	<table>
		<tbody>
			{#each items as item (item.slug)}
				<tr>
					<td>{item.name}</td>
					<td>
						<select
							aria-label={`presence of ${item.name}`}
							value={item.presence}
							onchange={(e) => tap('pantry-set', { item: item.slug, presence: e.currentTarget.value })}
						>
							<option value="have">have</option>
							<option value="low">low</option>
							<option value="out">out</option>
						</select>
					</td>
					<td>
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
					</td>
					<td>
						<button
							class="secondary outline"
							title="Remove entirely (usually you want presence: out)"
							onclick={() => tap('pantry-remove', { item: item.slug })}>✕</button
						>
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
	<form onsubmit={addItem} role="group">
		<input placeholder="New item (e.g. chicken thighs)" bind:value={newItem} />
		<button type="submit" disabled={!newItem.trim()}>Add</button>
	</form>
{/if}

<style>
	td {
		vertical-align: middle;
	}
	td select,
	td button {
		margin-bottom: 0;
	}
</style>
