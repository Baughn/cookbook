// M6: pantry recon — a photo draws a proposal, taps apply it, words
// correct it. The scripted model reads the shelf wrong on purpose; the
// flow's whole job is making that harmless.

import { enter, expect, expectNoSidewaysScroll, test } from './helpers';

// A 4×4 grey PNG — the fake model never looks at pixels, the client's
// downscale pipeline just needs a decodable image.
const PNG = Buffer.from(
	'iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAADklEQVR4nGNoQAIMxHEAcFIYAYPG8BkAAAAASUVORK5CYII=',
	'base64'
);

test('photo → proposal → tap → correction → revised proposal', async ({ page }) => {
	await enter(page, '/page/locations/home/pantry');
	await expect(page.getByRole('heading', { name: 'Thread' })).toBeVisible();

	// Attach two photos across two picks — a shelf rarely fits one frame,
	// so picks accumulate instead of replacing. The camera button only
	// exists on pantry pages.
	await page.getByLabel('shelf photo').setInputFiles({
		name: 'shelf-left.png',
		mimeType: 'image/png',
		buffer: PNG
	});
	await page.getByLabel('shelf photo').setInputFiles({
		name: 'shelf-right.png',
		mimeType: 'image/png',
		buffer: PNG
	});
	await expect(page.getByRole('button', { name: 'attach photo' })).toHaveText('📷2');
	await page.getByPlaceholder('Snap the shelf', { exact: false }).fill('here is the shelf');
	await page.getByRole('button', { name: 'Send' }).click();

	// The proposal card arrives; nothing has been applied.
	const card = page.getByLabel('recon proposal');
	await expect(card).toBeVisible();
	await expect(card.getByText('no jar visible')).toBeVisible();
	await expect(page.getByText('Proposed pantry updates', { exact: false })).toBeVisible();
	// The transcript stored a counted placeholder, not pixels.
	await expect(page.getByText('[2 photos attached]').first()).toBeVisible();
	// Proposing alone changed nothing: open the editor — miso isn't there.
	await page.getByRole('button', { name: 'Edit', exact: true }).click();
	// Wait for the editor to load before measuring positions below: its
	// pop-in is the initial render of a view the user just asked for, not
	// a tap moving things.
	await expect(page.getByPlaceholder('New item', { exact: false })).toBeVisible();
	await expect(page.getByLabel('presence of miso')).toHaveCount(0);
	// The proposal card, Apply buttons and all, fits a phone.
	await expectNoSidewaysScroll(page);

	// One tap applies one line, as an ordinary ui: edit — and the line the
	// finger is on stays put: the page refreshes in place, no remount, no
	// scroll jump. (Tolerance covers the pantry editor above growing by
	// the applied row.)
	const applyMiso = card.getByLabel('apply miso out');
	await applyMiso.scrollIntoViewIfNeeded();
	const before = (await applyMiso.boundingBox())!;
	await applyMiso.click();
	await expect(card.getByLabel('applied miso')).toBeVisible();
	await expect(page.getByLabel('presence of miso')).toHaveValue('out');
	const after = (await card.getByLabel('applied miso').boundingBox())!;
	expect(Math.abs(after.y - before.y)).toBeLessThanOrEqual(50);
	// The un-tapped line stayed un-applied.
	await expect(page.getByLabel('presence of rice')).toHaveCount(0);
	await page.getByText('Recent changes', { exact: false }).click();
	await expect(page.getByText('ui: pantry home: set miso').first()).toBeVisible();

	// Mere conversation doesn't take the Apply buttons away: the assistant
	// replies, the un-applied lines stay tappable, the applied stay ✓.
	await page.getByPlaceholder('Snap the shelf', { exact: false }).fill('thanks');
	await page.getByRole('button', { name: 'Send' }).click();
	await expect(page.getByText('Anytime.')).toBeVisible();
	await expect(card.getByLabel('apply rice have')).toBeVisible();
	await expect(card.getByLabel('applied miso')).toBeVisible();

	// Nor does a tab reload (phones discard tabs constantly): the latest
	// proposal lives on the server until completed or superseded, and
	// applied-ness is read off the pantry itself.
	await page.reload();
	await expect(card.getByLabel('applied miso')).toBeVisible();
	await expect(card.getByLabel('apply rice have')).toBeVisible();

	// A correction is just words on the thread; it draws a fresh proposal,
	// which supersedes the old one outright.
	await page.getByPlaceholder('Snap the shelf', { exact: false }).fill('you missed the dashi');
	await page.getByRole('button', { name: 'Send' }).click();
	await expect(card.getByText('taking your word')).toBeVisible();
	await expect(card.getByText('no jar visible')).toHaveCount(0);
	await card.getByRole('button', { name: /Apply all/ }).click();
	await expect(card.getByText('✓ All applied.')).toBeVisible();
	await page.getByRole('button', { name: 'Edit', exact: true }).click();
	await expect(page.getByLabel('presence of dashi')).toHaveValue('have');

	// Completed proposals are done for good: a reload shows a clean thread.
	await page.reload();
	await expect(page.getByRole('heading', { name: 'Thread' })).toBeVisible();
	await expect(page.getByLabel('recon proposal')).toHaveCount(0);
});

test('a mid-batch failure survives Apply all and the line can be retried', async ({ page }) => {
	await enter(page, '/page/locations/home/pantry');
	await expect(page.getByRole('heading', { name: 'Thread' })).toBeVisible();

	// The previous spec (when it ran — the corpus is shared) left miso
	// `out`, which would pre-apply its proposal line: reset it so the
	// photo draws two tappable lines. On a fresh corpus miso doesn't
	// exist yet and there is nothing to reset.
	await page.getByRole('button', { name: 'Edit', exact: true }).click();
	await expect(page.getByPlaceholder('New item', { exact: false })).toBeVisible();
	if (await page.getByLabel('presence of miso').count()) {
		await page.getByLabel('presence of miso').selectOption('have');
		// A reload proves the change persisted server-side.
		await page.reload();
		await page.getByRole('button', { name: 'Edit', exact: true }).click();
		await expect(page.getByLabel('presence of miso')).toHaveValue('have');
	}

	await page.getByLabel('shelf photo').setInputFiles({
		name: 'shelf.png',
		mimeType: 'image/png',
		buffer: PNG
	});
	await page.getByPlaceholder('Snap the shelf', { exact: false }).fill('the shelf again');
	await page.getByRole('button', { name: 'Send' }).click();
	const card = page.getByLabel('recon proposal');
	await expect(card.getByLabel('apply miso out')).toBeVisible();
	await expect(card.getByLabel('apply rice have')).toBeVisible();

	// miso's pantry-set fails; rice's succeeds. The batch runs in line
	// order (miso first), so the success lands *after* the failure — the
	// case where a loop that resets its banner per line shows two ✓s, no
	// error, and a pantry still claiming miso is on the shelf.
	await page.route('**/api/edit/pantry-set', async (route) => {
		if (route.request().postDataJSON()?.item === 'miso') {
			await route.fulfill({ status: 500, contentType: 'text/plain', body: 'boom' });
		} else {
			await route.continue();
		}
	});
	await card.getByRole('button', { name: /Apply all/ }).click();

	await expect(card.getByLabel('applied rice')).toBeVisible();
	await expect(card.getByLabel('failed miso')).toBeVisible();
	await expect(card.getByText('1 of 2 applied', { exact: false })).toBeVisible();
	// The failed line keeps its Apply button: the fix is one tap away.
	await expect(card.getByLabel('apply miso out')).toBeVisible();

	// And once the server recovers, that tap completes the proposal.
	await page.unroute('**/api/edit/pantry-set');
	await card.getByLabel('apply miso out').click();
	await expect(card.getByText('✓ All applied.')).toBeVisible();
});
