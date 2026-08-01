import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api, chat, Unauthorized } from './api';

// The spec is one sentence: "One token prompt, localStorage, 401 loops back
// to it." The loop-back lives in request() — the one path every API call
// takes — so no call site can forget it. These tests pin that: a 401
// clears the stored token and reloads (the layout gate keys on the token),
// whatever the endpoint.

function fakeStorage() {
	const store = new Map<string, string>();
	return {
		getItem: (k: string) => store.get(k) ?? null,
		setItem: (k: string, v: string) => void store.set(k, v),
		removeItem: (k: string) => void store.delete(k),
		has: (k: string) => store.has(k)
	};
}

let storage: ReturnType<typeof fakeStorage>;
let reload: ReturnType<typeof vi.fn>;

beforeEach(() => {
	storage = fakeStorage();
	storage.setItem('mise-token', 'a-perfectly-fine-token');
	reload = vi.fn();
	vi.stubGlobal('localStorage', storage);
	vi.stubGlobal('location', { reload });
});

afterEach(() => {
	vi.unstubAllGlobals();
});

function respond(status: number, body: unknown = {}) {
	vi.stubGlobal(
		'fetch',
		vi.fn(async () => ({
			status,
			ok: status >= 200 && status < 300,
			statusText: String(status),
			json: async () => body
		}))
	);
}

describe('the 401 loop-back', () => {
	it('clears the token and returns to the gate from any call site', async () => {
		respond(401);
		await expect(api.queue()).rejects.toBeInstanceOf(Unauthorized);
		expect(storage.has('mise-token'), 'token cleared').toBe(false);
		expect(reload).toHaveBeenCalled();

		storage.setItem('mise-token', 'a-perfectly-fine-token');
		await expect(api.edit('pantry-set', { item: 'miso' })).rejects.toBeInstanceOf(Unauthorized);
		expect(storage.has('mise-token'), 'token cleared on POST paths too').toBe(false);
	});

	it('leaves the token alone on success and on ordinary errors', async () => {
		respond(200, { entries: [] });
		await api.queue();
		expect(storage.has('mise-token')).toBe(true);
		expect(reload).not.toHaveBeenCalled();

		respond(500);
		await expect(api.queue()).rejects.toThrow();
		expect(storage.has('mise-token'), 'a server error is not a bad token').toBe(true);
		expect(reload).not.toHaveBeenCalled();
	});
});

// chat() is the one streaming path. Its cleanup contract: the reader is
// always cancelled — on abort, and on a malformed frame that throws out
// of the loop — so an abandoned exchange cannot keep streaming into a
// thread the user has left.

function streamingFetch(chunks: string[], opts: { hold?: boolean } = {}) {
	const encoder = new TextEncoder();
	const pending = [...chunks];
	let signal: AbortSignal | undefined;
	let stallReject: ((e: unknown) => void) | null = null;
	const aborted = () => new DOMException('aborted', 'AbortError');
	const reader = {
		// Mirrors a real fetch reader: an aborted signal rejects the
		// current and every subsequent read.
		read: vi.fn(
			() =>
				new Promise<{ done: boolean; value?: Uint8Array }>((resolve, reject) => {
					if (signal?.aborted) reject(aborted());
					else if (pending.length)
						resolve({ done: false, value: encoder.encode(pending.shift()!) });
					else if (opts.hold) stallReject = reject;
					else resolve({ done: true });
				})
		),
		cancel: vi.fn(async () => {})
	};
	vi.stubGlobal(
		'fetch',
		vi.fn(async (_path: string, init?: RequestInit) => {
			signal = init?.signal ?? undefined;
			signal?.addEventListener('abort', () => stallReject?.(aborted()));
			return {
				status: 200,
				ok: true,
				statusText: 'OK',
				json: async () => ({}),
				body: { getReader: () => reader }
			};
		})
	);
	return reader;
}

describe('chat stream cleanup', () => {
	it('an aborted exchange cancels the reader', async () => {
		const reader = streamingFetch(['event: delta\ndata: {"text":"thinking…"}\n\n'], {
			hold: true
		});
		const controller = new AbortController();
		const deltas: string[] = [];
		const exchange = chat(
			'hello',
			null,
			{
				onDelta: (t) => {
					deltas.push(t);
					controller.abort();
				},
				onTool: () => {},
				onDone: () => {},
				onError: () => {}
			},
			[],
			controller.signal
		);
		await expect(exchange).rejects.toMatchObject({ name: 'AbortError' });
		expect(deltas).toEqual(['thinking…']);
		expect(reader.cancel).toHaveBeenCalled();
	});

	it('a malformed frame still cancels the reader on the way out', async () => {
		const reader = streamingFetch(['event: delta\ndata: not json\n\n']);
		const exchange = chat('hello', null, {
			onDelta: () => {},
			onTool: () => {},
			onDone: () => {},
			onError: () => {}
		});
		await expect(exchange).rejects.toThrow();
		expect(reader.cancel).toHaveBeenCalled();
	});
});
