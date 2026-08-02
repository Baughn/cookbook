// M5: the cookbook front door, the new-recipe draft flow, and direct
// item editing — all against the real server and a scripted model.

import { enter, expect, test } from './helpers';

test('cookbook: drafts land on the shelf, first tap promotes', async ({ page }) => {
	await enter(page, '/cookbook');
	await expect(page.getByRole('heading', { name: 'Cookbook' })).toBeVisible();
	// The seeded recipe is on the active shelf with its tag chip.
	await expect(page.getByRole('link', { name: 'Mapo tofu' })).toBeVisible();
	await expect(page.getByRole('button', { name: /cuisine=sichuan/ })).toBeVisible();

	// Ask for a draft; the scripted model recipe_adds tonkatsu as draft.
	await page.getByPlaceholder('Describe a dish', { exact: false }).fill('tonkatsu, from that video');
	await page.getByRole('button', { name: 'Draft' }).click();
	// The reply stays on screen — an exchange that ends with a question
	// must not vanish — and the fresh draft is linked from the box.
	await expect(page.getByText('Drafted tonkatsu.')).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Drafts' })).toBeVisible();
	const draftLink = page.getByRole('link', { name: 'Tonkatsu', exact: true }).first();
	await expect(draftLink).toBeVisible();

	// The conversation lives on the drafting thread; planning stays clean.
	await page.goto('/page/threads/drafting');
	await expect(page.getByText('tonkatsu, from that video')).toBeVisible();
	await page.goto('/');
	// Anchor the absence on a completed planning exchange: right after
	// goto the thread is still empty, where "not visible" is vacuously
	// true and reverting the thread separation would stay green.
	await page.getByPlaceholder('Plan the week', { exact: false }).fill('thanks');
	await page.getByRole('button', { name: 'Send' }).click();
	await expect(page.getByText('Anytime.')).toBeVisible();
	await expect(page.getByText('tonkatsu, from that video')).not.toBeVisible();

	// The draft's page carries status taps; promoting moves it up.
	await page.goto('/cookbook');
	await page.getByRole('link', { name: 'Tonkatsu', exact: true }).first().click();
	await expect(page.getByRole('heading', { name: 'Tonkatsu', exact: true })).toBeVisible();
	const statusGroup = page.getByRole('group', { name: 'recipe status' });
	await expect(statusGroup.getByRole('button', { name: 'draft' })).toBeVisible();
	await statusGroup.getByRole('button', { name: 'active' }).click();
	await expect(page.locator('.frontmatter')).toContainText('status active');

	await page.goto('/cookbook');
	// The positive first: Tonkatsu on the active shelf proves the page
	// data arrived before the Drafts heading's absence means anything.
	await expect(page.getByRole('link', { name: 'Tonkatsu' })).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Drafts' })).not.toBeVisible();
});

test('pantry and equipment edit in place, with ui provenance', async ({ page }) => {
	// The nav's Pantry resolves the active location's page.
	await enter(page, '/');
	await page.getByRole('link', { name: 'Pantry', exact: true }).click();
	await expect(page).toHaveURL(/\/page\/locations\/home\/pantry/);

	// One representation at a time: the page opens on the rendered export,
	// and the editor replaces it behind the Edit toggle. The rendered
	// heading anchors the load — count(0) against a page still saying
	// "Loading…" would pass even if the editor rendered by default.
	await expect(page.getByRole('heading', { name: /Pantry/ })).toBeVisible();
	await expect(page.getByPlaceholder('New item', { exact: false })).toHaveCount(0);
	await page.getByRole('button', { name: 'Edit', exact: true }).click();
	await page.getByPlaceholder('New item', { exact: false }).fill('silken tofu');
	await page.getByRole('button', { name: 'Add', exact: true }).click();
	const presence = page.getByLabel('presence of silken tofu');
	await expect(presence).toHaveValue('have');
	await presence.selectOption('low');
	await expect(page.getByLabel('presence of silken tofu')).toHaveValue('low');

	// The tap is ordinary history on the page, under ui provenance.
	await page.getByText('Recent changes', { exact: false }).click();
	await expect(page.getByText('ui: pantry home: set silken-tofu').first()).toBeVisible();

	// Equipment: chips with in-place add, behind the same toggle.
	await page.getByRole('link', { name: 'Equipment', exact: true }).click();
	await expect(page).toHaveURL(/\/page\/locations\/home\/equipment/);
	await page.getByRole('button', { name: 'Edit', exact: true }).click();
	await page.getByPlaceholder('New equipment', { exact: false }).fill('stand mixer');
	await page.getByRole('button', { name: 'Add', exact: true }).click();
	await expect(page.getByRole('button', { name: 'stand-mixer ✕' })).toBeVisible();
});

test('edit only appears where editing works', async ({ page }) => {
	// The active location's pantry page offers the toggle.
	await enter(page, '/page/locations/home/pantry');
	await expect(page.getByRole('button', { name: 'Edit' })).toBeVisible();

	// The cabin is not active: same page shape, no toggle — the editors
	// refuse non-active locations, so a toggle here blanks the page.
	await page.goto('/page/locations/cabin/pantry');
	await expect(page.getByRole('heading', { name: /Pantry/ })).toBeVisible();
	await expect(page.getByRole('button', { name: 'Edit' })).not.toBeVisible();
});
