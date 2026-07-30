<script lang="ts">
	// The raw corpus, every exported path — the debug corner. The curated
	// front door is /cookbook.
	import { api, clearToken, Unauthorized } from '$lib/api';
	import type { PageInfo } from '$lib/types';

	let pages: PageInfo[] = $state([]);
	let error: string | null = $state(null);

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
</script>

<hgroup>
	<h2>All pages</h2>
	<p>Every file in the export, unfiltered.</p>
</hgroup>
{#if error}
	<article><p>⚠ {error}</p></article>
{:else}
	<ul>
		{#each pages as p (p.path)}
			<li>
				<a href={`/page/${p.path.replace(/\.md$/, '')}`}>{p.path}</a>
				{#if p.status && p.status !== 'active'}<small>[{p.status}]</small>{/if}
			</li>
		{/each}
	</ul>
{/if}
