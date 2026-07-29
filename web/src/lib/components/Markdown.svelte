<script lang="ts">
	import { marked } from 'marked';

	let { content }: { content: string } = $props();
	let html = $derived(marked.parse(content, { async: false }) as string);
</script>

<!-- The export is our own render output, not third-party input. -->
<div class="markdown">{@html html}</div>

<style>
	.markdown :global(table) {
		display: block;
		overflow-x: auto;
	}
</style>
