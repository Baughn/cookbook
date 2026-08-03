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

function codePoint(n: number): string {
	try {
		return String.fromCodePoint(n);
	} catch {
		return '';
	}
}

/**
 * Resolve numeric HTML character references (`&#106;`, `&#x6a;`, semicolon
 * optional as browsers allow), so the scheme test below sees what the browser
 * will decode in the emitted `href` — not the raw text. `&#106;avascript:`
 * and `javascript&#58;` are `javascript:` to a browser.
 */
function decodeNumericRefs(s: string): string {
	return s
		.replace(/&#x([0-9a-f]+);?/gi, (_, h) => codePoint(parseInt(h, 16)))
		.replace(/&#(\d+);?/g, (_, d) => codePoint(parseInt(d, 10)));
}

/**
 * Relative URLs and fragments pass through. A URL carrying a scheme must carry
 * one of ours — `javascript:`, `data:` and `vbscript:` are the ones that
 * execute, but an allowlist means a scheme we have not thought about cannot
 * surprise us either.
 *
 * The subtlety is that `{@html}` lets the browser decode character references
 * inside the `href` attribute, so validating the raw text is validating the
 * wrong string. Two moves close that: resolve numeric refs and drop
 * characters at or below U+0020 (browsers ignore both inside a scheme, so
 * `java<TAB>script:` and `&#9;` are working spellings a prefix check misses)
 * *before* the test; and escape every `&` on the way out, so no reference —
 * numeric or named — can re-form in the attribute. What the browser decodes is
 * then exactly what was validated, nothing more.
 */
export function safeUrl(href: string): string {
	const bare = Array.from(decodeNumericRefs(href))
		.filter((c) => c.charCodeAt(0) > 0x20)
		.join('');
	const scheme = bare.match(HAS_SCHEME);
	if (scheme && !SAFE_SCHEME.test(scheme[1].toLowerCase())) return '';
	return bare.replace(/&/g, '&amp;');
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
