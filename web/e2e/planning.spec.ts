// The M4 deliverable: the planning-session flow from the design doc, end
// to end in a browser — against the real server and a scripted model.

import { enter, expect, test } from './helpers';

test('planning session: token, queue, chat, edits land', async ({ page }) => {
	// One kitchen, one token.
	await enter(page, '/');

	// The queue home shows readiness against the location.
	await expect(page.getByRole('heading', { name: 'Queue — home' })).toBeVisible();
	await expect(page.getByText('Mapo tofu')).toBeVisible();
	await expect(page.getByText('missing equipment here: wok')).toBeVisible();
	await expect(page.getByText('nothing cooked')).toBeVisible();

	// Ask the planning thread; the scripted model queues dal and replies.
	// The composer is a real textarea: Shift+Enter breaks the line, Enter
	// sends (on a keyboard — phones keep Enter as newline).
	const composer = page.getByPlaceholder('Plan the week…');
	await composer.fill('plan something');
	await composer.press('Shift+Enter');
	await composer.pressSequentially('cheap');
	await expect(composer).toHaveValue('plan something\ncheap');
	await composer.press('Enter');
	await expect(page.getByText('Queued dal.')).toBeVisible();
	// Both lines landed in the one sent message.
	await expect(page.getByText('plan something cheap')).toBeVisible();

	// The exchange's edit landed and the queue reloaded.
	await expect(page.getByText('Dal', { exact: true })).toBeVisible();
	await expect(page.getByText('why: cheap')).toBeVisible();
});

test('recipe page: rendered markdown, history, thread', async ({ page }) => {
	await enter(page, '/');
	await page.goto('/page/recipes/mapo-tofu');
	await expect(page.getByRole('heading', { name: 'Mapo tofu', exact: true })).toBeVisible();
	// Frontmatter renders as metadata, not as an accidental heading.
	await expect(page.getByText('schema-version')).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Ingredients' })).toBeVisible();

	// Doc-backed pages carry their history and their own thread.
	await page.getByText('Recent changes', { exact: false }).click();
	await expect(page.getByText('cli: recipe add mapo-tofu')).toBeVisible();
	await expect(page.getByPlaceholder('Ask about this page…')).toBeVisible();
});

// Audit #62: the recipe-status side fetch (/api/pages, the slower of the
// page's two fetches) used to route its failure into the page-level
// `error`, an exclusive template branch — so a failed decoration replaced
// the already-rendered recipe, its editors and its thread with a bare ⚠
// banner. Losing the status must cost the status row, nothing else.
test('a failed status fetch costs the status row, not the page', async ({ page }) => {
	await enter(page, '/');
	// /api/pages walks the whole corpus and is the slower of the page's two
	// fetches, so the ordinary ordering is content first, then the status
	// answer. Hold the 500 until the page has painted to pin that order —
	// an immediate rejection loses the race and hides the bug.
	let releasePages!: () => void;
	const gate = new Promise<void>((r) => (releasePages = r));
	await page.route('**/api/pages', async (route) => {
		await gate;
		await route.fulfill({ status: 500, body: '{"error":"transient store lock"}' });
	});
	await page.goto('/page/recipes/mapo-tofu');

	const heading = page.getByRole('heading', { name: 'Mapo tofu', exact: true });
	await expect(heading).toBeVisible();
	releasePages();

	// The page stays in full; only the decoration is missing.
	await expect(page.getByText('transient store lock')).toHaveCount(0);
	await expect(heading).toBeVisible();
	await expect(page.getByPlaceholder('Ask about this page…')).toBeVisible();
	await expect(page.getByRole('group', { name: 'recipe status' })).toHaveCount(0);
	await page.unroute('**/api/pages');
});
