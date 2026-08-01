<script lang="ts">
	import '@picocss/pico/css/pico.min.css';
	import { getToken, setToken } from '$lib/api';

	let { children } = $props();
	let token = $state(getToken());
	let draft = $state('');
	let checking = $state(false);
	let gateError = $state('');

	// The gate stores nothing unverified: one fat-fingered character must
	// not dismiss the prompt permanently. A raw fetch, not api.request —
	// the candidate token isn't stored yet, and a 401 here is the gate's
	// own answer, not a loop back to itself.
	async function save(e: SubmitEvent) {
		e.preventDefault();
		const candidate = draft.trim();
		if (candidate.length < 16) return;
		checking = true;
		gateError = '';
		try {
			const r = await fetch('/api/location', {
				headers: { authorization: `Bearer ${candidate}` }
			});
			if (r.status === 401) {
				gateError = 'The server refused that token.';
				return;
			}
			setToken(candidate);
			token = candidate;
		} catch {
			gateError = 'Could not reach the server to check the token.';
		} finally {
			checking = false;
		}
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
				<button type="submit" disabled={checking || draft.trim().length < 16}>
					{checking ? 'Checking…' : 'Enter'}
				</button>
			</form>
			{#if gateError}
				<p>{gateError}</p>
			{/if}
		</article>
	{/if}
</main>

<style>
	/* Five links fit a phone by wrapping, never by overflowing. */
	nav ul {
		flex-wrap: wrap;
	}
</style>
