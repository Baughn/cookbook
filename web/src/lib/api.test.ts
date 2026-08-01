import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api, Unauthorized } from './api';

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
