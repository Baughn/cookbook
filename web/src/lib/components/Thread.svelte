<script lang="ts">
	import { api, chat, type ChatImage } from '$lib/api';
	import { downscaleAll } from '$lib/photo';
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

	// One in-flight exchange at most. The generation counter makes stale
	// closures inert: a thread change bumps it, so callbacks from an
	// abandoned exchange stop touching this component's state.
	let exchange = 0;
	let controller: AbortController | null = null;

	async function reload() {
		const r = await api.thread(thread);
		messages = r.messages;
		// The thread's live proposal (the server keeps the latest until
		// completed or superseded) — but never downgrade one already
		// streamed in: the server omits completed proposals, and the ✓s
		// of one finished this session should stay on screen.
		proposal = r.proposal ?? proposal;
		error = null;
	}

	$effect(() => {
		void thread;
		// A thread change abandons any in-flight exchange and its
		// on-screen leftovers — streaming text, tool notes, errors all
		// belonged to the previous thread.
		controller?.abort();
		exchange++;
		busy = false;
		streaming = '';
		toolNotes = [];
		error = null;
		proposal = null;
		reload().catch((e) => (error = String(e)));
		return () => controller?.abort();
	});

	function photoNote(count: number): string {
		return count === 1 ? '[photo attached]' : `[${count} photos attached]`;
	}

	// One exchange, driven from the composer. A throw anywhere before the
	// exchange lands rejects back into the composer, which restores the
	// message and the photos. An abandoned exchange (thread change,
	// unmount) resolves quietly — its thread keeps the message server-side
	// if it arrived, and the new thread owes it nothing.
	async function send(message: string, files: File[]) {
		if (busy) return;
		const gen = ++exchange;
		const ctl = new AbortController();
		controller = ctl;
		busy = true;
		error = null;
		streaming = '';
		toolNotes = [];
		// The proposal stays: mere conversation doesn't take the Apply
		// buttons away. A fresh proposal replaces it via onProposal.
		const live = () => gen === exchange;
		try {
			let images: ChatImage[] = [];
			if (files.length > 0) {
				images = await downscaleAll(files);
			}
			// Mirror the server's transcript placeholder until reload.
			const shown = images.length
				? message
					? `${message}\n\n${photoNote(images.length)}`
					: photoNote(images.length)
				: message;
			// The empty `created` marks it as unconfirmed until reload.
			messages = [...messages, { role: 'user', content: shown, created: '' }];
			await chat(
				message,
				thread === 'planning' ? null : thread,
				{
					onDelta: (text) => live() && (streaming += text),
					onTool: (name) => live() && (toolNotes = [...toolNotes, name]),
					onProposal: (p) => live() && (proposal = p),
					onDone: () => {},
					onError: (message) => live() && (error = message)
				},
				images,
				ctl.signal
			);
			if (!live()) return;
			await reload();
			onExchangeDone?.();
		} catch (e) {
			if (!live() || ctl.signal.aborted) return;
			// The failed optimistic bubble comes off the transcript — the
			// message goes back to the composer instead of showing twice.
			// (Unconfirmed bubbles are the ones without a server stamp;
			// $state proxies rule out removal by object identity.)
			messages = messages.filter((m) => m.created !== '');
			error = String(e);
			throw e;
		} finally {
			if (live()) {
				busy = false;
				streaming = '';
				toolNotes = [];
			}
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
