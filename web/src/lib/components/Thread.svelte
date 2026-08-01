<script lang="ts">
	import { api, chat, type ChatImage } from '$lib/api';
	import { downscale } from '$lib/photo';
	import Composer from '$lib/components/Composer.svelte';
	import ReconProposal from '$lib/components/ReconProposal.svelte';
	import type { ReconProposal as Proposal, ThreadMessage } from '$lib/types';

	let {
		thread,
		onExchangeDone,
		photos = false
	}: {
		thread: string;
		onExchangeDone?: () => void;
		/// Offer the camera — pantry pages, where a photo means recon.
		photos?: boolean;
	} = $props();

	let messages: ThreadMessage[] = $state([]);
	let busy = $state(false);
	let streaming = $state('');
	let toolNotes: string[] = $state([]);
	let error: string | null = $state(null);
	let proposal: Proposal | null = $state(null);

	async function reload() {
		const r = await api.thread(thread);
		messages = r.messages;
		// The thread's live proposal (the server keeps the latest until
		// completed or superseded) — but never downgrade one already
		// streamed in: the server omits completed proposals, and the ✓s
		// of one finished this session should stay on screen.
		proposal = r.proposal ?? proposal;
	}

	$effect(() => {
		void thread;
		proposal = null;
		reload().catch((e) => (error = String(e)));
	});

	function photoNote(count: number): string {
		return count === 1 ? '[photo attached]' : `[${count} photos attached]`;
	}

	// One exchange, driven from the composer. A throw anywhere before the
	// exchange lands rejects back into the composer, which restores the
	// message and the photos.
	async function send(message: string, files: File[]) {
		if (busy) return;
		busy = true;
		error = null;
		streaming = '';
		toolNotes = [];
		// The proposal stays: mere conversation doesn't take the Apply
		// buttons away. A fresh proposal replaces it via onProposal.
		try {
			let images: ChatImage[] = [];
			if (files.length > 0) {
				images = await Promise.all(files.map(downscale));
			}
			// Mirror the server's transcript placeholder until reload.
			const shown = images.length
				? message
					? `${message}\n\n${photoNote(images.length)}`
					: photoNote(images.length)
				: message;
			messages = [...messages, { role: 'user', content: shown, created: '' }];
			await chat(
				message,
				thread === 'planning' ? null : thread,
				{
					onDelta: (text) => (streaming += text),
					onTool: (name) => (toolNotes = [...toolNotes, name]),
					onProposal: (p) => (proposal = p),
					onDone: () => {},
					onError: (message) => (error = message)
				},
				images
			);
			await reload();
			onExchangeDone?.();
		} catch (e) {
			error = String(e);
			throw e;
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
	{#if proposal}
		<ReconProposal {proposal} onApplied={() => onExchangeDone?.()} />
	{/if}
	{#if error}
		<article class="error"><p>⚠ {error}</p></article>
	{/if}
	<Composer
		placeholder={photos
			? 'Snap the shelf, or ask…'
			: thread === 'planning'
				? 'Plan the week…'
				: 'Ask about this page…'}
		{busy}
		{photos}
		onSend={send}
	/>
</section>

<style>
	article.user {
		margin-left: 2rem;
	}
	article.error {
		border-color: var(--pico-del-color, #b71c1c);
	}
</style>
