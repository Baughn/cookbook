<script lang="ts">
	import { api, chat, clearToken, Unauthorized } from '$lib/api';
	import type { PageInfo } from '$lib/types';

	let pages: PageInfo[] = $state([]);
	let error: string | null = $state(null);
	let filter: string | null = $state(null);

	// The new-recipe flow speaks to the planning thread; once the draft
	// page exists, its own thread takes over.
	let draft = $state('');
	let busy = $state(false);
	let streaming = $state('');
	let toolNotes: string[] = $state([]);

	async function reload() {
		pages = (await api.pages()).pages;
	}

	$effect(() => {
		reload().catch((e) => {
			if (e instanceof Unauthorized) {
				clearToken();
				location.reload();
				return;
			}
			error = String(e);
		});
	});

	async function draftRecipe(e: SubmitEvent) {
		e.preventDefault();
		const what = draft.trim();
		if (!what || busy) return;
		busy = true;
		error = null;
		streaming = '';
		toolNotes = [];
		try {
			await chat(`Draft a new recipe for the cookbook: ${what}`, null, {
				onDelta: (text) => (streaming += text),
				onTool: (name) => (toolNotes = [...toolNotes, name]),
				onDone: () => {},
				onError: (message) => (error = message)
			});
			draft = '';
			await reload();
		} catch (e) {
			error = String(e);
		} finally {
			busy = false;
		}
	}

	let recipes = $derived(pages.filter((p) => p.path.startsWith('recipes/')));
	let active = $derived(recipes.filter((r) => (r.status ?? 'active') === 'active'));
	let drafts = $derived(recipes.filter((r) => r.status === 'draft'));
	let retired = $derived(recipes.filter((r) => r.status === 'retired'));
	let techniques = $derived(pages.filter((p) => p.path.startsWith('techniques/')));

	/// Every tag on the active shelf as "axis=value", by frequency.
	let tags = $derived.by(() => {
		const counts = new Map<string, number>();
		for (const r of active) {
			for (const [axis, value] of Object.entries(r.tags ?? {})) {
				const key = `${axis}=${value}`;
				counts.set(key, (counts.get(key) ?? 0) + 1);
			}
		}
		return [...counts.entries()].sort((a, b) => b[1] - a[1]);
	});

	let shown = $derived(
		filter
			? active.filter((r) => Object.entries(r.tags ?? {}).some(([a, v]) => `${a}=${v}` === filter))
			: active
	);

	function link(r: PageInfo): string {
		return `/page/${r.path.replace(/\.md$/, '')}`;
	}
</script>

<hgroup>
	<h2>Cookbook</h2>
	<p>What this kitchen makes.</p>
</hgroup>

{#if error}
	<article><p>⚠ {error}</p></article>
{/if}

<p>
	{#each tags as [tag, count] (tag)}
		<button
			class={filter === tag ? '' : 'outline'}
			style="margin: 0 0.3rem 0.3rem 0; padding: 0.1rem 0.6rem"
			onclick={() => (filter = filter === tag ? null : tag)}
		>
			{tag} <small>({count})</small>
		</button>
	{/each}
</p>
<ul>
	{#each shown as r (r.path)}
		<li>
			<a href={link(r)}>{r.title ?? r.path}</a>
			<small>
				[{r.effort}]
				{Object.entries(r.tags ?? {})
					.map(([a, v]) => `${a}=${v}`)
					.join(' ')}
			</small>
		</li>
	{/each}
</ul>

{#if drafts.length > 0}
	<h3>Drafts</h3>
	<p><small>Never cooked yet — the first logged cook moves them up.</small></p>
	<ul>
		{#each drafts as r (r.path)}
			<li><a href={link(r)}>{r.title ?? r.path}</a></li>
		{/each}
	</ul>
{/if}

<h3>New recipe</h3>
<form onsubmit={draftRecipe}>
	<div role="group">
		<input
			type="text"
			placeholder="Describe a dish, or paste a URL…"
			bind:value={draft}
			disabled={busy}
		/>
		<button type="submit" disabled={busy}>Draft</button>
	</div>
</form>
{#if busy}
	<article aria-busy="true">
		{#each toolNotes as note, i (i)}
			<small>⚙ {note}</small><br />
		{/each}
		<p style="white-space: pre-wrap">{streaming}</p>
	</article>
{/if}

{#if techniques.length > 0}
	<h3>Techniques</h3>
	<ul>
		{#each techniques as t (t.path)}
			<li><a href={link(t)}>{t.title ?? t.path}</a></li>
		{/each}
	</ul>
{/if}

{#if retired.length > 0}
	<details>
		<summary>Retired ({retired.length})</summary>
		<ul>
			{#each retired as r (r.path)}
				<li><a href={link(r)}>{r.title ?? r.path}</a></li>
			{/each}
		</ul>
	</details>
{/if}
