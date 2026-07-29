import { describe, expect, it } from 'vitest';
import { SseFrames } from './sse';

describe('SseFrames', () => {
	it('survives arbitrary chunk boundaries', () => {
		const raw =
			'event: delta\ndata: {"text":"He"}\n\n' +
			'event: tool\ndata: {"name":"queue_add"}\n\n' +
			'event: done\ndata: {"reply":\ndata: "ok"}\n\n';
		// One character at a time — the cruellest chunking.
		const frames = new SseFrames();
		const got = [];
		for (const c of raw) got.push(...frames.push(c));
		expect(got).toEqual([
			{ event: 'delta', data: '{"text":"He"}' },
			{ event: 'tool', data: '{"name":"queue_add"}' },
			{ event: 'done', data: '{"reply":\n"ok"}' }
		]);
	});

	it('ignores keep-alive comments and empty frames', () => {
		const frames = new SseFrames();
		expect(frames.push(': keep-alive\n\n')).toEqual([]);
		expect(frames.push('event: delta\ndata: {}\n\n')).toEqual([
			{ event: 'delta', data: '{}' }
		]);
	});
});
