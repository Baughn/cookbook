import { Marked } from 'marked';

// Corpus pages are authored as Markdown and rendered into the DOM with
// {@html}. Their content is not ours: fetch_url pulls arbitrary third-party
// pages, extraction copies JSON-LD fields and Readability output verbatim into
// a recipe body, and the export writes that body byte-for-byte. So the render
// step is a trust boundary, and marked has shipped no sanitizer since v8.
//
// Three vectors, all live by default:
//   - inline raw HTML   `tofu <img src=x onerror=...>`
//   - block raw HTML    `<script>...</script>`
//   - scheme-bearing URLs in links and images  `[x](javascript:...)`
//
// No corpus page needs inline HTML — the export renders Markdown and nothing
// else — so raw HTML becomes visible text rather than markup. That also keeps
// the app and the export agreeing about what a document says.

const ESCAPES: Record<string, string> = {
	'&': '&amp;',
	'<': '&lt;',
	'>': '&gt;',
	'"': '&quot;',
	"'": '&#39;'
};

function escapeHtml(raw: string): string {
	return raw.replace(/[&<>"']/g, (c) => ESCAPES[c]);
}

const SAFE_SCHEME = /^(?:https?|mailto):$/;
const HAS_SCHEME = /^([a-z][a-z0-9+.-]*:)/i;

/**
 * Relative URLs and fragments pass through. A URL carrying a scheme must carry
 * one of ours — `javascript:`, `data:` and `vbscript:` are the ones that
 * execute, but an allowlist means a scheme we have not thought about cannot
 * surprise us either.
 *
 * Characters at or below U+0020 are dropped before the test: browsers ignore
 * them inside a scheme, so `java<TAB>script:` is a working spelling that a
 * naive prefix check misses.
 */
export function safeUrl(href: string): string {
	const bare = Array.from(href)
		.filter((c) => c.charCodeAt(0) > 0x20)
		.join('');
	const scheme = bare.match(HAS_SCHEME);
	if (scheme && !SAFE_SCHEME.test(scheme[1].toLowerCase())) return '';
	return href;
}

const marked = new Marked({
	walkTokens(token) {
		if (token.type === 'link' || token.type === 'image') {
			token.href = safeUrl(token.href ?? '');
		}
	},
	renderer: {
		html({ text }) {
			return escapeHtml(text);
		}
	}
});

/** Render corpus Markdown to HTML that is safe to inject with {@html}. */
export function renderMarkdown(body: string): string {
	return marked.parse(body, { async: false }) as string;
}
