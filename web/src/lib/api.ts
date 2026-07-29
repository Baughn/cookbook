// The typed client for the mise server. One static bearer token, kept in
// localStorage after first entry; 401 anywhere sends the user back to the
// token prompt.

import type { ChangeInfo, PageInfo, QueueView, ThreadMessage } from './types';
import { SseFrames } from './sse';

const TOKEN_KEY = 'mise-token';

export function getToken(): string | null {
	return localStorage.getItem(TOKEN_KEY);
}

export function setToken(token: string) {
	localStorage.setItem(TOKEN_KEY, token.trim());
}

export function clearToken() {
	localStorage.removeItem(TOKEN_KEY);
}

export class Unauthorized extends Error {
	constructor() {
		super('bad or missing token');
	}
}

async function request(path: string, init?: RequestInit): Promise<Response> {
	const response = await fetch(path, {
		...init,
		headers: {
			...init?.headers,
			authorization: `Bearer ${getToken() ?? ''}`
		}
	});
	if (response.status === 401) throw new Unauthorized();
	if (!response.ok) {
		let detail = '';
		try {
			detail = (await response.json()).error ?? '';
		} catch {
			// non-JSON error body; the status is all we have
		}
		throw new Error(detail || `${response.status} ${response.statusText}`);
	}
	return response;
}

async function getJson<T>(path: string): Promise<T> {
	return (await request(path)).json();
}

export const api = {
	queue: () => getJson<QueueView>('/api/queue'),
	pages: () => getJson<{ pages: PageInfo[] }>('/api/pages'),
	page: (path: string) => getJson<{ path: string; content: string }>(`/api/page/${path}`),
	history: (doc: string) =>
		getJson<{ doc: string; changes: ChangeInfo[] }>(`/api/history/${doc}`),
	thread: (thread: string) =>
		getJson<{ thread: string; messages: ThreadMessage[] }>(`/api/thread/${thread}`),
	revert: async (doc: string, hash: string) => {
		await request('/api/revert', {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ doc, hash })
		});
	}
};

export interface ChatEvents {
	onDelta: (text: string) => void;
	onTool: (name: string) => void;
	onDone: (reply: string) => void;
	onError: (message: string) => void;
}

/// One chat exchange, streamed. `page` omitted = the planning thread.
export async function chat(message: string, page: string | null, events: ChatEvents) {
	const response = await request('/chat', {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify(page ? { message, page } : { message })
	});
	const reader = response.body!.getReader();
	const decoder = new TextDecoder();
	const frames = new SseFrames();
	for (;;) {
		const { done, value } = await reader.read();
		if (done) break;
		for (const frame of frames.push(decoder.decode(value, { stream: true }))) {
			const data = frame.data ? JSON.parse(frame.data) : {};
			if (frame.event === 'delta') events.onDelta(data.text ?? '');
			else if (frame.event === 'tool') events.onTool(data.name ?? '?');
			else if (frame.event === 'done') events.onDone(data.reply ?? '');
			else if (frame.event === 'error') events.onError(data.message ?? 'unknown error');
		}
	}
}
