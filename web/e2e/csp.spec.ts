// The Content-Security-Policy is split: script-src/object-src/base-uri ride in
// the page as a build-time meta tag (the server can't know this build's inline
// bootstrap hash), while frame-ancestors and friends come from mise-server's
// response headers. Only the header half had a test — auth.rs checks its own
// hand-written index.html — so the meta, which is the last line of defence
// behind the {@html} sink, would boot identically with no policy at all. This
// asserts the real build carries it.
//
// Uses the bare Playwright test (no shared corpus, no overflow fixture): it
// only reads a meta tag off index.html.
import { expect, test } from '@playwright/test';

test('the built app ships a script-src CSP meta with no inline escape', async ({ page }) => {
	await page.goto('/');
	const csp = await page
		.locator('meta[http-equiv="content-security-policy" i]')
		.getAttribute('content');

	expect(csp, 'the CSP meta tag must be present in the build').toBeTruthy();
	const policy = csp ?? '';
	// Just the script-src directive (up to the next ';'), so style-src's
	// deliberate 'unsafe-inline' can't be mistaken for a script escape.
	const scriptSrc = policy.match(/script-src([^;]*)/i)?.[1] ?? '';
	expect(scriptSrc, `script-src missing from: ${policy}`).toBeTruthy();
	expect(scriptSrc).toMatch(/'self'/);
	expect(scriptSrc, 'the inline bootstrap must be hash-pinned').toMatch(/'sha256-/);
	expect(scriptSrc).not.toMatch(/'unsafe-inline'/);
	expect(policy).toMatch(/object-src[^;]*'none'/i);
	expect(policy).toMatch(/base-uri[^;]*'self'/i);
});
