<script lang="ts">
	import '@picocss/pico/css/pico.min.css';
	import { getToken, setToken } from '$lib/api';

	let { children } = $props();
	let token = $state(getToken());
	let draft = $state('');

	function save(e: SubmitEvent) {
		e.preventDefault();
		if (draft.trim().length < 16) return;
		setToken(draft);
		token = draft.trim();
	}
</script>

<nav class="container">
	<ul>
		<li><strong><a href="/">Mise</a></strong></li>
	</ul>
	<ul>
		<li><a href="/">Queue</a></li>
		<li><a href="/cookbook">Cookbook</a></li>
		<li><a href="/pantry">Pantry</a></li>
		<li><a href="/equipment">Equipment</a></li>
		<li><a href="/browse"><small>All pages</small></a></li>
	</ul>
</nav>

<main class="container">
	{#if token}
		{@render children()}
	{:else}
		<article>
			<header>One kitchen, one token</header>
			<form onsubmit={save}>
				<input
					type="password"
					placeholder="Bearer token (at least 16 characters)"
					bind:value={draft}
				/>
				<button type="submit" disabled={draft.trim().length < 16}>Enter</button>
			</form>
		</article>
	{/if}
</main>

<style>
	/* Five links fit a phone by wrapping, never by overflowing. */
	nav ul {
		flex-wrap: wrap;
	}
</style>
