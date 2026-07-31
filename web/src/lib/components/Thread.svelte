<script lang="ts">
	import { api, chat, type ChatImage } from '$lib/api';
	import { downscale } from '$lib/photo';
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
	let draft = $state('');
	let composer: HTMLTextAreaElement | undefined = $state();
	let busy = $state(false);
	let streaming = $state('');
	let toolNotes: string[] = $state([]);
	let error: string | null = $state(null);
	// A recon rarely fits one frame: picks accumulate until send.
	let photoFiles: File[] = $state([]);
	let photoInput: HTMLInputElement | undefined = $state();
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

	async function send(e: SubmitEvent) {
		e.preventDefault();
		const message = draft.trim();
		if ((!message && photoFiles.length === 0) || busy) return;
		draft = '';
		busy = true;
		error = null;
		streaming = '';
		toolNotes = [];
		// The proposal stays: mere conversation doesn't take the Apply
		// buttons away. A fresh proposal replaces it via onProposal.
		try {
			let images: ChatImage[] = [];
			if (photoFiles.length > 0) {
				images = await Promise.all(photoFiles.map(downscale));
				photoFiles = [];
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
		} finally {
			busy = false;
			streaming = '';
			toolNotes = [];
		}
	}

	function pickPhoto(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		photoFiles = [...photoFiles, ...Array.from(input.files ?? [])];
		input.value = '';
	}

	// Hardware keyboards send on Enter and break lines on Shift+Enter; a
	// phone keyboard's return key keeps making newlines — the Send button
	// is right there.
	const enterSends = !window.matchMedia('(pointer: coarse)').matches;

	function composerKeydown(e: KeyboardEvent) {
		if (enterSends && e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
			e.preventDefault();
			(e.currentTarget as HTMLTextAreaElement).form?.requestSubmit();
		}
	}

	// The composer grows with its text and shrinks back when sent.
	$effect(() => {
		void draft;
		if (!composer) return;
		composer.style.height = 'auto';
		composer.style.height = `${composer.scrollHeight}px`;
	});
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
	<form onsubmit={send}>
		<div role="group">
			{#if photos}
				<input
					bind:this={photoInput}
					type="file"
					accept="image/*"
					capture="environment"
					multiple
					onchange={pickPhoto}
					aria-label="shelf photo"
					style="display: none"
				/>
				<button
					type="button"
					class={photoFiles.length ? '' : 'outline'}
					title="Snap the shelf — snap again for another frame"
					aria-label="attach photo"
					disabled={busy}
					onclick={() => photoInput?.click()}
				>
					📷{photoFiles.length > 1 ? photoFiles.length : ''}
				</button>
			{/if}
			<textarea
				bind:this={composer}
				rows="1"
				placeholder={photos
					? 'Snap the shelf, or ask…'
					: thread === 'planning'
						? 'Plan the week…'
						: 'Ask about this page…'}
				bind:value={draft}
				disabled={busy}
				onkeydown={composerKeydown}
			></textarea>
			<button type="submit" disabled={busy || (!draft.trim() && photoFiles.length === 0)}>
				Send
			</button>
		</div>
		{#if photoFiles.length > 0}
			<small>
				Sent with your next message, kept only for this exchange:
				{#each photoFiles as file, i (i)}
					<span style="white-space: nowrap">
						📷 {file.name}
						<button
							type="button"
							class="outline"
							style="width: auto; padding: 0 0.4rem"
							aria-label={`remove ${file.name}`}
							onclick={() => (photoFiles = photoFiles.filter((_, j) => j !== i))}
						>
							✕
						</button>
					</span>{' '}
				{/each}
			</small>
		{/if}
	</form>
</section>

<style>
	article.user {
		margin-left: 2rem;
	}
	article.error {
		border-color: var(--pico-del-color, #b71c1c);
	}
	div[role='group'] > button:first-child {
		width: auto;
		flex-grow: 0;
	}
	div[role='group'] textarea {
		resize: none;
		overflow: hidden;
		margin-bottom: 0;
	}
</style>
