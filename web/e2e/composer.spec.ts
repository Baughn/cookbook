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

	// The failure is reported, and the work is still there.
	await expect(page.getByText('⚠', { exact: false })).toBeVisible();
	await expect(composer).toHaveValue('plan something nice');

	// The connection comes back; the same send now lands.
	await page.unroute('**/chat');
	await composer.press('Enter');
	await expect(page.getByText('Queued dal.')).toBeVisible();
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
});
