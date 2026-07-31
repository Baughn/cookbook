<script lang="ts">
	import { renderMarkdown } from '$lib/markdown';

	let { content }: { content: string } = $props();

	// Exported pages open with `---` frontmatter, which marked would read
	// as a setext heading; show it as metadata instead.
	function split(raw: string): { meta: [string, string][]; body: string } {
		if (!raw.startsWith('---\n')) return { meta: [], body: raw };
		const end = raw.indexOf('\n---\n', 4);
		if (end === -1) return { meta: [], body: raw };
		const meta = raw
			.slice(4, end)
			.split('\n')
			.map((line): [string, string] => {
				const i = line.indexOf(': ');
				return i === -1 ? [line.replace(/:$/, ''), ''] : [line.slice(0, i), line.slice(i + 2)];
			});
		return { meta, body: raw.slice(end + 5) };
	}

	let parts = $derived(split(content));
	let html = $derived(renderMarkdown(parts.body));
</script>

{#if parts.meta.length > 0}
	<p class="frontmatter">
		{#each parts.meta as [key, value] (key)}
			<small>
				<b>{key}</b>
				{#if value.startsWith('http://') || value.startsWith('https://')}
					<a href={value} rel="noreferrer external">{value}</a>
				{:else}
					{value}
				{/if}&ensp;
			</small>
		{/each}
	</p>
{/if}
<!-- Safe to inject: renderMarkdown escapes raw HTML and refuses executable
     URL schemes. Page content is transitively third-party — fetch_url drafts
     recipes from arbitrary sites — so it is never trusted here. -->
<div class="markdown">{@html html}</div>

<style>
	.markdown :global(table) {
		display: block;
		overflow-x: auto;
	}
</style>
