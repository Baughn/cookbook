<script lang="ts">
	import { page as route } from '$app/state';
	import { api } from '$lib/api';
	import EquipmentEditor from '$lib/components/EquipmentEditor.svelte';
	import History from '$lib/components/History.svelte';
	import Markdown from '$lib/components/Markdown.svelte';
	import PantryEditor from '$lib/components/PantryEditor.svelte';
	import Thread from '$lib/components/Thread.svelte';

	let content: string | null = $state(null);
	// Bumped on every reload; children re-fetch into existing DOM. Remount
	// ({#key}) is banned as a refresh mechanism — it collapses the layout
	// for a beat and throws the scroll position.
	let version = $state(0);
	let error: string | null = $state(null);
	// Pantry/equipment pages show ONE representation at a time: the
	// rendered export to read, the editor to change. Both at once was the
	// same data twice on screen.
	let editing = $state(false);

	let path = $derived(route.params.path ?? '');

	// The inverse of the server's DocId::export_path — doc-backed paths get
	// history, revert, and a page thread; log shards and transcripts don't.
	function docFor(p: string): string | null {
		const stem = p.replace(/\.md$/, '');
		if (['state', 'queue', 'someday', 'shopping', 'steering', 'facts'].includes(stem)) {
			return stem;
		}
		let m = stem.match(/^locations\/([a-z0-9-]+)\/(pantry|equipment|shops|fridge)$/);
		if (m) return `location/${m[1]}/${m[2]}`;
		m = stem.match(/^recipes\/([a-z0-9-]+)$/);
		if (m) return `recipe/${m[1]}`;
		m = stem.match(/^techniques\/([a-z0-9-]+)$/);
		if (m) return `technique/${m[1]}`;
		return null;
	}

	let doc = $derived(docFor(path));

	// Structured pages carry their editor in place; recipes their status.
	let editorLocation = $derived.by(() => {
		const m = (doc ?? '').match(/^location\/([a-z0-9-]+)\/(pantry|equipment)$/);
		return m ? { location: m[1], kind: m[2] } : null;
	});
	let recipeSlug = $derived(doc?.startsWith('recipe/') ? doc.slice('recipe/'.length) : null);
	let status: string | null = $state(null);

	// The status row drives recipe-status POSTs for the *current* slug, so
	// a slow response for a page we've left must never paint it: capture
	// the slug at effect entry and discard mismatched answers.
	$effect(() => {
		status = null;
		const slug = recipeSlug;
		if (!slug) return;
		const wanted = `recipes/${slug}.md`;
		api
			.pages()
			.then((r) => {
				if (recipeSlug !== slug) return;
				status = r.pages.find((p) => p.path === wanted)?.status ?? null;
			})
			// The status row is a decoration, already hidden when null —
			// losing it costs the row, never the page. Routing this into
			// `error` replaced the rendered recipe, editors and thread
			// with a bare banner (the template branches exclusively).
			.catch(() => (status = null));
	});

	// The Edit toggle only appears where editing works: the editors
	// refuse non-active locations, so offering the toggle there blanked
	// the page.
	let activeLocation: string | null = $state(null);
	let editable = $derived(
		editorLocation !== null && editorLocation.location === activeLocation
	);

	async function setStatus(next: string) {
		try {
			await api.edit('recipe-status', { slug: recipeSlug, status: next });
			status = next;
			await reload();
		} catch (e) {
			error = String(e);
		}
	}

	async function reload() {
		try {
			content = (await api.page(path)).content;
			version += 1;
			error = null;
		} catch (e) {
			error = String(e);
		}
	}

	$effect(() => {
		void path;
		editing = false;
		reload();
		api
			.location()
			.then((l) => (activeLocation = l.location))
			.catch(() => (activeLocation = null));
	});
</script>

{#if error}
	<article><p>⚠ {error}</p></article>
{:else if content !== null}
	{#if recipeSlug && status}
		<p role="group" aria-label="recipe status">
			{#each ['draft', 'active', 'retired'] as s (s)}
				<button
					class={status === s ? '' : 'outline'}
					style="padding: 0.1rem 0.6rem; width: auto"
					onclick={() => status !== s && setStatus(s)}
				>
					{s}
				</button>
			{/each}
		</p>
	{/if}
	{#if editorLocation && editable}
		<p class="mode">
			<button class="outline" onclick={() => (editing = !editing)}>
				{editing ? 'Done' : 'Edit'}
			</button>
		</p>
	{/if}
	<!-- Edits from anywhere (taps, recon, revert) bump version; editors
	     and history reload in place, so nothing on screen moves. -->
	{#if editorLocation && editable && editing}
		{#if editorLocation.kind === 'pantry'}
			<PantryEditor location={editorLocation.location} {version} onChanged={reload} />
		{:else}
			<EquipmentEditor location={editorLocation.location} {version} onChanged={reload} />
		{/if}
	{:else}
		<Markdown {content} />
	{/if}
	{#if doc}
		<hr />
		<History {doc} {version} onReverted={reload} />
		<h3>Thread</h3>
		<Thread thread={doc} onExchangeDone={reload} photos={editorLocation?.kind === 'pantry'} />
	{/if}
{:else}
	<article aria-busy="true">Loading…</article>
{/if}

<style>
	.mode {
		display: flex;
		justify-content: flex-end;
		margin-bottom: 0.5rem;
	}
	.mode button {
		width: auto;
		margin-bottom: 0;
		padding: 0.15rem 0.8rem;
	}
</style>
