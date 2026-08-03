// The composer contract, shared by the thread and the drafting box: a
// failed send is an inconvenience, not a loss. The motivating environment
// is a shop basement with no signal — the typed message must come back.

import { enter, expect, test } from './helpers';

test('a failed send puts the message back in the composer', async ({ page }) => {
	await enter(page, '/');
	const composer = page.getByPlaceholder('Plan the week…');

	// The network dies before the exchange starts.
	await page.route('**/chat', (route) => route.abort());
	await composer.fill('plan something nice');
	await composer.press('Enter');

	// The failure is reported, and the work is still there — in the
	// composer, not as a ghost bubble in the transcript.
	await expect(page.getByText('⚠', { exact: false })).toBeVisible();
	await expect(composer).toHaveValue('plan something nice');
	await expect(page.locator('article.user', { hasText: 'plan something nice' })).toHaveCount(0);

	// The connection comes back; the same send lands and the error clears.
	await page.unroute('**/chat');
	await composer.press('Enter');
	await expect(page.getByText('Queued dal.')).toBeVisible();
	await expect(page.locator('article.user', { hasText: 'plan something nice' })).toHaveCount(1);
	await expect(page.getByText('⚠', { exact: false })).not.toBeVisible();
});

test('the drafting box keeps its message on failure too', async ({ page }) => {
	await enter(page, '/cookbook');
	// The placeholder flips to "Answer, or refine…" once a turn exists.
	const box = page.getByPlaceholder(/Describe a dish|Answer, or refine/);

	await page.route('**/chat', (route) => route.abort());
	await box.fill('tonkatsu, from that video');
	await page.getByRole('button', { name: 'Draft' }).click();

	await expect(page.getByText('⚠', { exact: false })).toBeVisible();
	await expect(box).toHaveValue('tonkatsu, from that video');
	await expect(page.locator('section[aria-label="drafting"] article.user')).toHaveCount(0);
});

// Audit #60: a failure the server reports over SSE ends the stream cleanly,
// so chat() resolves and send() takes the success path — where reload()
// used to clear the error banner in the same tick onError set it. `done`
// and `error` frames are mutually exclusive server-side, so *every*
// server-side exchange failure took this path: the banner never rendered.
test('a server-reported exchange failure keeps its banner', async ({ page }) => {
	await enter(page, '/');
	const composer = page.getByPlaceholder('Plan the week…');

	await page.route('**/chat', (route) =>
		route.fulfill({
			status: 200,
			headers: { 'content-type': 'text/event-stream' },
			body: 'event: error\ndata: {"message":"the exchange failed: model overloaded"}\n\n'
		})
	);
	await composer.fill('plan something doomed');
	await composer.press('Enter');

	// The banner survives the transcript reload that follows the stream.
	await expect(page.getByText('model overloaded', { exact: false })).toBeVisible();
	await page.unroute('**/chat');
});

// Audit #67: the drafting box's failed-turn cleanup removed every user turn
// whose text equalled the failed one — and short refinements repeat
// naturally ("spicier", "again"), so a failed repeat erased the earlier
// successful turn too, orphaning the assistant reply between them. Removal
// must key on a structural marker (the turn this send pushed), not on text.
test('a failed repeat removes only itself from the drafting session', async ({ page }) => {
	await enter(page, '/cookbook');
	const box = page.getByPlaceholder(/Describe a dish|Answer, or refine/);
	const drafting = page.locator('section[aria-label="drafting"]');
	const send = page.getByRole('button', { name: 'Draft' });

	// A refinement lands…
	await box.fill('spicier');
	await send.click();
	await expect(drafting.locator('article.assistant').last()).toBeVisible();

	// …and the identical follow-up fails in transit.
	await page.route('**/chat', (route) => route.abort());
	await box.fill('spicier');
	await send.click();
	await expect(page.getByText('⚠', { exact: false })).toBeVisible();

	// The failed turn is back in the box; the earlier, confirmed one stays.
	await expect(box).toHaveValue('spicier');
	await expect(drafting.locator('article.user', { hasText: 'spicier' })).toHaveCount(1);
	await page.unroute('**/chat');
});
