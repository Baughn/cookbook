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
