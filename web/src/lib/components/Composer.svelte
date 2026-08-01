<script lang="ts">
	// The one composer, shared by the thread and the drafting box. The
	// contract: Enter sends and Shift+Enter breaks lines on hardware
	// keyboards; on coarse-pointer devices the return key keeps making
	// newlines and the button sends. The box grows with its text. A failed
	// send is an inconvenience, not a loss — the message and any attached
	// photos come back.

	let {
		placeholder,
		busy = false,
		photos = false,
		sendLabel = 'Send',
		onSend
	}: {
		placeholder: string;
		busy?: boolean;
		/// Offer the camera — pantry pages, where a photo means recon.
		photos?: boolean;
		sendLabel?: string;
		/// Perform one exchange. A rejection puts the message and the
		/// photos back in the composer; resolving consumes them.
		onSend: (message: string, files: File[]) => Promise<void>;
	} = $props();

	import { MAX_PHOTOS } from '$lib/photo';

	let draft = $state('');
	let files: File[] = $state([]);
	let area: HTMLTextAreaElement | undefined = $state();
	let photoInput: HTMLInputElement | undefined = $state();

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		const message = draft.trim();
		if ((!message && files.length === 0) || busy) return;
		// Clear optimistically — the sent text lives in the transcript
		// while streaming, not in the box — but restore on rejection:
		// a typed message or a shelf photo is not repeatable on a phone
		// in a signal-dead basement.
		const sent = files;
		draft = '';
		files = [];
		try {
			await onSend(message, sent);
		} catch {
			draft = message;
			files = sent;
		}
	}

	function pickPhoto(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		// Cap at what the server admits per exchange — nothing gets
		// downscaled, uploaded and then refused.
		files = [...files, ...Array.from(input.files ?? [])].slice(0, MAX_PHOTOS);
		input.value = '';
	}

	// Hardware keyboards send on Enter and break lines on Shift+Enter; a
	// phone keyboard's return key keeps making newlines — the Send button
	// is right there.
	const enterSends = !window.matchMedia('(pointer: coarse)').matches;

	function keydown(e: KeyboardEvent) {
		if (enterSends && e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
			e.preventDefault();
			(e.currentTarget as HTMLTextAreaElement).form?.requestSubmit();
		}
	}

	// The composer grows with its text and shrinks back when sent.
	$effect(() => {
		void draft;
		if (!area) return;
		area.style.height = 'auto';
		area.style.height = `${area.scrollHeight}px`;
	});
</script>

<form onsubmit={submit}>
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
				class={files.length ? '' : 'outline'}
				title="Snap the shelf — snap again for another frame"
				aria-label="attach photo"
				disabled={busy}
				onclick={() => photoInput?.click()}
			>
				📷{files.length > 1 ? files.length : ''}
			</button>
		{/if}
		<textarea
			bind:this={area}
			rows="1"
			{placeholder}
			bind:value={draft}
			disabled={busy}
			onkeydown={keydown}
		></textarea>
		<button type="submit" disabled={busy || (!draft.trim() && files.length === 0)}>
			{sendLabel}
		</button>
	</div>
	{#if files.length > 0}
		<small>
			Sent with your next message, kept only for this exchange:
			{#each files as file, i (i)}
				<span style="white-space: nowrap">
					📷 {file.name}
					<button
						type="button"
						class="outline"
						style="width: auto; padding: 0 0.4rem"
						aria-label={`remove ${file.name}`}
						onclick={() => (files = files.filter((_, j) => j !== i))}
					>
						✕
					</button>
				</span>{' '}
			{/each}
		</small>
	{/if}
</form>

<style>
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
