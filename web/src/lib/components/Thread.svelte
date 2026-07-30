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
	let busy = $state(false);
	let streaming = $state('');
	let toolNotes: string[] = $state([]);
	let error: string | null = $state(null);
	let photoFile: File | null = $state(null);
	let photoInput: HTMLInputElement | undefined = $state();
	let proposal: Proposal | null = $state(null);

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
		if ((!message && !photoFile) || busy) return;
		draft = '';
		busy = true;
		error = null;
		streaming = '';
		toolNotes = [];
		proposal = null;
		let image: ChatImage | undefined;
		try {
			if (photoFile) {
				image = await downscale(photoFile);
				photoFile = null;
			}
			// Mirror the server's transcript placeholder until reload.
			const shown = image ? (message ? `${message}\n\n[photo attached]` : '[photo attached]') : message;
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
				image
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
		photoFile = input.files?.[0] ?? null;
		input.value = '';
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
	<form onsubmit={send}>
		<div role="group">
			{#if photos}
				<input
					bind:this={photoInput}
					type="file"
					accept="image/*"
					capture="environment"
					onchange={pickPhoto}
					aria-label="shelf photo"
					style="display: none"
				/>
				<button
					type="button"
					class={photoFile ? '' : 'outline'}
					title={photoFile ? `attached: ${photoFile.name} (tap to replace)` : 'Snap the shelf'}
					aria-label="attach photo"
					disabled={busy}
					onclick={() => photoInput?.click()}
				>
					📷
				</button>
			{/if}
			<input
				type="text"
				placeholder={photos
					? 'Snap the shelf, or ask…'
					: thread === 'planning'
						? 'Plan the week…'
						: 'Ask about this page…'}
				bind:value={draft}
				disabled={busy}
			/>
			<button type="submit" disabled={busy || (!draft.trim() && !photoFile)}>Send</button>
		</div>
		{#if photoFile}
			<small>
				📷 {photoFile.name} attached — sent with your next message, kept only for this exchange.
				<button type="button" class="outline" style="width: auto; padding: 0 0.4rem" onclick={() => (photoFile = null)}>
					✕
				</button>
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
</style>
