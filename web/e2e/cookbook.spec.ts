// M5: the cookbook front door, the new-recipe draft flow, and direct
// item editing — all against the real server and a scripted model.

import { expect, test } from '@playwright/test';

const TOKEN = 'e2e-token-0123456789abcdef';

async function enter(page: import('@playwright/test').Page, path: string) {
	await page.goto(path);
	const gate = page.getByPlaceholder('Bearer token', { exact: false });
	if (await gate.isVisible()) {
		await gate.fill(TOKEN);
		await page.getByRole('button', { name: 'Enter' }).click();
	}
}

test('cookbook: drafts land on the shelf, first tap promotes', async ({ page }) => {
	await enter(page, '/cookbook');
	await expect(page.getByRole('heading', { name: 'Cookbook' })).toBeVisible();
	// The seeded recipe is on the active shelf with its tag chip.
	await expect(page.getByRole('link', { name: 'Mapo tofu' })).toBeVisible();
	await expect(page.getByRole('button', { name: /cuisine=sichuan/ })).toBeVisible();

	// Ask for a draft; the scripted model recipe_adds tonkatsu as draft.
	await page.getByPlaceholder('Describe a dish', { exact: false }).fill('tonkatsu, from that video');
	await page.getByRole('button', { name: 'Draft' }).click();
	await expect(page.getByRole('heading', { name: 'Drafts' })).toBeVisible();
	const draftLink = page.getByRole('link', { name: 'Tonkatsu' });
	await expect(draftLink).toBeVisible();

	// The draft's page carries status taps; promoting moves it up.
	await draftLink.click();
	await expect(page.getByRole('heading', { name: 'Tonkatsu', exact: true })).toBeVisible();
	const statusGroup = page.getByRole('group', { name: 'recipe status' });
	await expect(statusGroup.getByRole('button', { name: 'draft' })).toBeVisible();
	await statusGroup.getByRole('button', { name: 'active' }).click();
	await expect(page.locator('.frontmatter')).toContainText('status active');

	await page.goto('/cookbook');
	await expect(page.getByRole('heading', { name: 'Drafts' })).not.toBeVisible();
	await expect(page.getByRole('link', { name: 'Tonkatsu' })).toBeVisible();
});

test('pantry and equipment edit in place, with ui provenance', async ({ page }) => {
	// The nav's Pantry resolves the active location's page.
	await enter(page, '/');
	await page.getByRole('link', { name: 'Pantry', exact: true }).click();
	await expect(page).toHaveURL(/\/page\/locations\/home\/pantry/);

	await page.getByPlaceholder('New item', { exact: false }).fill('silken tofu');
	await page.getByRole('button', { name: 'Add', exact: true }).click();
	const presence = page.getByLabel('presence of silken tofu');
	await expect(presence).toHaveValue('have');
	await presence.selectOption('low');
	await expect(page.getByLabel('presence of silken tofu')).toHaveValue('low');

	// The tap is ordinary history on the page, under ui provenance.
	await page.getByText('Recent changes', { exact: false }).click();
	await expect(page.getByText('ui: pantry home: set silken-tofu').first()).toBeVisible();

	// Equipment: chips with in-place add.
	await page.getByRole('link', { name: 'Equipment', exact: true }).click();
	await expect(page).toHaveURL(/\/page\/locations\/home\/equipment/);
	await page.getByPlaceholder('New equipment', { exact: false }).fill('stand mixer');
	await page.getByRole('button', { name: 'Add', exact: true }).click();
	await expect(page.getByRole('button', { name: 'stand-mixer ✕' })).toBeVisible();
});
