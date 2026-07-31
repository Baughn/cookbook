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
	}

	// A tap reports upward; the page bumps `version` and everything
	// refreshes in place. One path, no remount.
	async function tap(action: string, item: string) {
		error = null;
		try {
			await api.edit(action, { item, location });
			onChanged();
		} catch (e) {
			error = String(e);
		}
	}

	function add(e: SubmitEvent) {
		e.preventDefault();
		const item = newItem.trim().toLowerCase().replace(/\s+/g, '-');
		if (!item) return;
		newItem = '';
		tap('equipment-set', item);
	}

	$effect(() => {
		void version;
		load().catch((e) => (error = String(e)));
	});

	// $derived.by: see PantryEditor — dodges TS's initializer narrowing.
	let editable = $derived.by(() => view?.location === location);
</script>

{#if error}
	<p>⚠ {error}</p>
{/if}
{#if view && editable}
	<p>
		{#each view.view.equipment as item (item)}
			<button
				class="outline"
				style="margin: 0 0.3rem 0.3rem 0; padding: 0.1rem 0.6rem"
				title={`Remove ${item}`}
				onclick={() => tap('equipment-remove', item)}
			>
				{item} ✕
			</button>
		{/each}
	</p>
	<form onsubmit={add} role="group">
		<input placeholder="New equipment (e.g. stand mixer)" bind:value={newItem} />
		<button type="submit" disabled={!newItem.trim()}>Add</button>
	</form>
{/if}
