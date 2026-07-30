<script lang="ts">
	import { api, clearToken, Unauthorized } from '$lib/api';
	import type { PageInfo } from '$lib/types';

	let pages: PageInfo[] = $state([]);
	let error: string | null = $state(null);
	let filter: string | null = $state(null);

	$effect(() => {
		api
			.pages()
			.then((r) => (pages = r.pages))
			.catch((e) => {
				if (e instanceof Unauthorized) {
					clearToken();
					location.reload();
					return;
				}
				error = String(e);
			});
	});

	let recipes = $derived(pages.filter((p) => p.path.startsWith('recipes/') && p.status !== 'retired'));
	let techniques = $derived(pages.filter((p) => p.path.startsWith('techniques/')));
	let others = $derived(
		pages.filter((p) => !p.path.startsWith('recipes/') && !p.path.startsWith('techniques/'))
	);

	/// Every tag as "axis=value", with counts, sorted by frequency.
	let tags = $derived.by(() => {
		const counts = new Map<string, number>();
		for (const r of recipes) {
			for (const [axis, value] of Object.entries(r.tags ?? {})) {
				const key = `${axis}=${value}`;
				counts.set(key, (counts.get(key) ?? 0) + 1);
			}
		}
		return [...counts.entries()].sort((a, b) => b[1] - a[1]);
	});

	let shown = $derived(
		filter
			? recipes.filter((r) =>
					Object.entries(r.tags ?? {}).some(([a, v]) => `${a}=${v}` === filter)
				)
			: recipes
	);
</script>

{#if error}
	<article><p>⚠ {error}</p></article>
{:else}
	<h2>Recipes</h2>
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
				<a href={`/page/${r.path.replace(/\.md$/, '')}`}>{r.title ?? r.path}</a>
				<small>
					[{r.effort}]
					{Object.entries(r.tags ?? {})
						.map(([a, v]) => `${a}=${v}`)
						.join(' ')}
				</small>
			</li>
		{/each}
	</ul>

	{#if techniques.length > 0}
		<h2>Techniques</h2>
		<ul>
			{#each techniques as t (t.path)}
				<li><a href={`/page/${t.path.replace(/\.md$/, '')}`}>{t.title ?? t.path}</a></li>
			{/each}
		</ul>
	{/if}

	<h2>Everything else</h2>
	<ul>
		{#each others as p (p.path)}
			<li><a href={`/page/${p.path.replace(/\.md$/, '')}`}>{p.path}</a></li>
		{/each}
	</ul>
{/if}
