<script lang="ts">
	import { page as route } from '$app/state';
	import { api, clearToken, Unauthorized } from '$lib/api';
	import History from '$lib/components/History.svelte';
	import Markdown from '$lib/components/Markdown.svelte';
	import Thread from '$lib/components/Thread.svelte';

	let content: string | null = $state(null);
	let error: string | null = $state(null);

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

	async function reload() {
		try {
			content = (await api.page(path)).content;
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
		void path;
		reload();
	});
</script>

{#if error}
	<article><p>⚠ {error}</p></article>
{:else if content !== null}
	<Markdown {content} />
	{#if doc}
		<hr />
		<History {doc} onReverted={reload} />
		<h3>Thread</h3>
		<Thread thread={doc} onExchangeDone={reload} />
	{/if}
{:else}
	<article aria-busy="true">Loading…</article>
{/if}
