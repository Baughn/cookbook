// Incremental SSE framing: chunks in, complete events out. The same shape
// as the server-side framer — events can split anywhere across chunks.

export interface SseEvent {
	event: string;
	data: string;
}

export class SseFrames {
	private buf = '';

	push(chunk: string): SseEvent[] {
		this.buf += chunk;
		const frames: SseEvent[] = [];
		let end;
		while ((end = this.buf.indexOf('\n\n')) !== -1) {
			const raw = this.buf.slice(0, end);
			this.buf = this.buf.slice(end + 2);
			let event = '';
			let data = '';
			for (const line of raw.split('\n')) {
				if (line.startsWith('event:')) {
					event = line.slice(6).trim();
				} else if (line.startsWith('data:')) {
					if (data !== '') data += '\n';
					data += line.slice(5).replace(/^ /, '');
				}
			}
			if (event !== '' || data !== '') frames.push({ event, data });
		}
		return frames;
	}
}
