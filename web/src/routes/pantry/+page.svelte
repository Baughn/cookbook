<script lang="ts">
	import { goto } from '$app/navigation';
	import { api } from '$lib/api';

	let error: string | null = $state(null);

	$effect(() => {
		api
			.location()
			.then((l) => goto(`/page/locations/${l.location}/pantry`, { replaceState: true }))
			.catch((e) => (error = String(e)));
	});
</script>

{#if error}
	<article><p>⚠ {error}</p></article>
{:else}
	<article aria-busy="true">Loading…</article>
{/if}
