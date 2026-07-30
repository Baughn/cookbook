// M6: pantry recon — a photo draws a proposal, taps apply it, words
// correct it. The scripted model reads the shelf wrong on purpose; the
// flow's whole job is making that harmless.

import { expect, test } from '@playwright/test';

const TOKEN = 'e2e-token-0123456789abcdef';

// A 4×4 grey PNG — the fake model never looks at pixels, the client's
// downscale pipeline just needs a decodable image.
const PNG = Buffer.from(
	'iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAADklEQVR4nGNoQAIMxHEAcFIYAYPG8BkAAAAASUVORK5CYII=',
	'base64'
);

async function enter(page: import('@playwright/test').Page, path: string) {
	await page.goto(path);
	const gate = page.getByPlaceholder('Bearer token', { exact: false });
	if (await gate.isVisible()) {
		await gate.fill(TOKEN);
		await page.getByRole('button', { name: 'Enter' }).click();
	}
}

test('photo → proposal → tap → correction → revised proposal', async ({ page }) => {
	await enter(page, '/page/locations/home/pantry');
	await expect(page.getByRole('heading', { name: 'Thread' })).toBeVisible();

	// Attach a photo and send. The camera button only exists on pantry pages.
	await page.getByLabel('shelf photo').setInputFiles({
		name: 'shelf.png',
		mimeType: 'image/png',
		buffer: PNG
	});
	await page.getByPlaceholder('Snap the shelf', { exact: false }).fill('here is the shelf');
	await page.getByRole('button', { name: 'Send' }).click();

	// The proposal card arrives; nothing has been applied.
	const card = page.getByLabel('recon proposal');
	await expect(card).toBeVisible();
	await expect(card.getByText('no jar visible')).toBeVisible();
	await expect(page.getByText('Proposed pantry updates', { exact: false })).toBeVisible();
	// The transcript stored a placeholder, not pixels.
	await expect(page.getByText('[photo attached]').first()).toBeVisible();
	// Proposing alone changed nothing: miso is not in the pantry editor.
	await expect(page.getByLabel('presence of miso')).toHaveCount(0);

	// One tap applies one line, as an ordinary ui: edit.
	await card.getByLabel('apply miso out').click();
	await expect(card.getByLabel('applied miso')).toBeVisible();
	await expect(page.getByLabel('presence of miso')).toHaveValue('out');
	// The un-tapped line stayed un-applied.
	await expect(page.getByLabel('presence of rice')).toHaveCount(0);
	await page.getByText('Recent changes', { exact: false }).click();
	await expect(page.getByText('ui: pantry home: set miso').first()).toBeVisible();

	// A correction is just words on the thread; it draws a fresh proposal.
	await page.getByPlaceholder('Snap the shelf', { exact: false }).fill('you missed the dashi');
	await page.getByRole('button', { name: 'Send' }).click();
	await expect(card.getByText('taking your word')).toBeVisible();
	await expect(card.getByText('no jar visible')).toHaveCount(0);
	await card.getByRole('button', { name: /Apply all/ }).click();
	await expect(page.getByLabel('presence of dashi')).toHaveValue('have');
});
