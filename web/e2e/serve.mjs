// E2E environment: a scripted fake Anthropic endpoint + the real
// mise-server on a seeded corpus, serving the built web app. Playwright's
// webServer runs this; no model is ever involved.

import { execFileSync, spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const TOKEN = 'e2e-token-0123456789abcdef';

// --- scripted model, dispatching on the conversation itself ---
const sse = (events) =>
	events.map(([event, data]) => `event: ${event}\ndata: ${data}\n\n`).join('');
const toolTurn = (name, input) =>
	sse([
		['message_start', '{}'],
		[
			'content_block_start',
			JSON.stringify({ content_block: { type: 'tool_use', id: 'c1', name } })
		],
		[
			'content_block_delta',
			JSON.stringify({ delta: { type: 'input_json_delta', partial_json: JSON.stringify(input) } })
		],
		['content_block_stop', '{}'],
		['message_delta', '{"delta":{"stop_reason":"tool_use"}}'],
		['message_stop', '{}']
	]);
const textTurn = (text) =>
	sse([
		['message_start', '{}'],
		['content_block_start', '{"content_block":{"type":"text","text":""}}'],
		['content_block_delta', JSON.stringify({ delta: { type: 'text_delta', text } })],
		['content_block_stop', '{}'],
		['message_delta', '{"delta":{"stop_reason":"end_turn"}}'],
		['message_stop', '{}']
	]);

const fake = createServer((req, res) => {
	let body = '';
	req.on('data', (chunk) => (body += chunk));
	req.on('end', () => {
		res.setHeader('content-type', 'text/event-stream');
		const request = JSON.parse(body);
		const last = request.messages.at(-1);
		const blocks = Array.isArray(last.content) ? last.content : [];
		const afterTools = blocks.some((b) => b.type === 'tool_result');
		// Dispatch on the latest actual question — threads accumulate, so
		// earlier questions must not leak into later exchanges.
		const asked = request.messages
			.filter((m) => m.role === 'user')
			.flatMap((m) => (Array.isArray(m.content) ? m.content : [{ type: 'text', text: m.content }]))
			.filter((b) => b.type === 'text')
			.map((b) => b.text)
			.at(-1) ?? '';
		if (afterTools) {
			res.end(textTurn(asked.includes('Draft a new recipe') ? 'Drafted tonkatsu.' : 'Queued dal.'));
		} else if (asked.includes('Draft a new recipe')) {
			res.end(
				toolTurn('recipe_add', {
					slug: 'tonkatsu',
					title: 'Tonkatsu',
					status: 'draft',
					tags: { cuisine: 'japanese' },
					body: 'Bread the pork. Fry it.'
				})
			);
		} else {
			res.end(toolTurn('queue_add', { title: 'Dal', reason: 'cheap' }));
		}
	});
});
await new Promise((resolve) => fake.listen(0, '127.0.0.1', resolve));
const fakeUrl = `http://127.0.0.1:${fake.address().port}`;

// --- build binaries and seed a corpus ---
const cargo = (args) => execFileSync('cargo', args, { cwd: repo, stdio: 'inherit' });
cargo(['build', '-q', '-p', 'mise-cli', '-p', 'mise-server']);
const root = join(mkdtempSync(join(tmpdir(), 'mise-e2e-')), 'corpus');
const mise = (args) =>
	execFileSync(join(repo, 'target/debug/mise'), ['--root', root, ...args], { stdio: 'inherit' });
mise(['init']);
mise([
	'recipe', 'add', 'mapo-tofu',
	'--title', 'Mapo tofu',
	'--servings', '4',
	'--tag', 'cuisine=sichuan',
	'--equipment', 'wok'
]);
mise(['queue', 'add', 'Mapo tofu', '--recipe', 'mapo-tofu', '--reason', 'craving']);

// --- the real server, serving the built app ---
const server = spawn(
	join(repo, 'target/debug/mise-server'),
	[
		'--root', root,
		'--listen', '127.0.0.1:7940',
		'--static-dir', join(repo, 'web/build'),
		'--anthropic-base-url', fakeUrl
	],
	{
		stdio: 'inherit',
		env: { ...process.env, MISE_TOKEN: TOKEN, ANTHROPIC_API_KEY: 'e2e-fake-key' }
	}
);
process.on('exit', () => server.kill());
server.on('exit', (code) => process.exit(code ?? 0));
