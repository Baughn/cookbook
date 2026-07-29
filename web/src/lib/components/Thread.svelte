<script lang="ts">
	import { api, chat } from '$lib/api';
	import type { ThreadMessage } from '$lib/types';

	let {
		thread,
		onExchangeDone
	}: { thread: string; onExchangeDone?: () => void } = $props();

	let messages: ThreadMessage[] = $state([]);
	let draft = $state('');
	let busy = $state(false);
	let streaming = $state('');
	let toolNotes: string[] = $state([]);
	let error: string | null = $state(null);

	async function reload() {
		messages = (await api.thread(thread)).messages;
	}

	$effect(() => {
		void thread;
		reload().catch((e) => (error = String(e)));
	});

	async function send(e: SubmitEvent) {
		e.preventDefault();
		const message = draft.trim();
		if (!message || busy) return;
		draft = '';
		busy = true;
		error = null;
		streaming = '';
		toolNotes = [];
		messages = [...messages, { role: 'user', content: message, created: '' }];
		try {
			await chat(message, thread === 'planning' ? null : thread, {
				onDelta: (text) => (streaming += text),
				onTool: (name) => (toolNotes = [...toolNotes, name]),
				onDone: () => {},
				onError: (message) => (error = message)
			});
			await reload();
			onExchangeDone?.();
		} catch (e) {
			error = String(e);
		} finally {
			busy = false;
			streaming = '';
			toolNotes = [];
		}
	}
</script>

<section aria-label="thread">
	{#each messages as m (m.created + m.role + m.content)}
		<article class={m.role}>
			<header><small>{m.role}{m.created ? ` — ${m.created.replace('T', ' ')}` : ''}</small></header>
			<p style="white-space: pre-wrap">{m.content}</p>
		</article>
	{/each}
	{#if busy}
		<article class="assistant" aria-busy="true">
			{#each toolNotes as note, i (i)}
				<small>⚙ {note}</small><br />
			{/each}
			<p style="white-space: pre-wrap">{streaming}</p>
		</article>
	{/if}
	{#if error}
		<article class="error"><p>⚠ {error}</p></article>
	{/if}
	<form onsubmit={send}>
		<div role="group">
			<input
				type="text"
				placeholder={thread === 'planning' ? 'Plan the week…' : 'Ask about this page…'}
				bind:value={draft}
				disabled={busy}
			/>
			<button type="submit" disabled={busy}>Send</button>
		</div>
	</form>
</section>

<style>
	article.user {
		margin-left: 2rem;
	}
	article.error {
		border-color: var(--pico-del-color, #b71c1c);
	}
</style>
