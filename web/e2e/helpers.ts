// Shared spec plumbing. The suite runs at a phone viewport (see
// playwright.config.ts), and every test ends by asserting the page fits
// it: horizontal overflow anywhere is a failing layout, not a nit to
// notice in the kitchen.

import { expect, test as base, type Page } from '@playwright/test';

export const TOKEN = 'e2e-token-0123456789abcdef';

export async function enter(page: Page, path: string) {
	await page.goto(path);
	const gate = page.getByPlaceholder('Bearer token', { exact: false });
	if (await gate.isVisible()) {
		await gate.fill(TOKEN);
		await page.getByRole('button', { name: 'Enter' }).click();
	}
}

export async function expectNoSidewaysScroll(page: Page) {
	// On failure, name the widest offenders — a bare "44px too wide" is
	// not actionable.
	const report = await page.evaluate(() => {
		const limit = document.documentElement.clientWidth;
		const overflow = document.documentElement.scrollWidth - limit;
		const scrollsInside = (el: Element): boolean => {
			for (let a = el.parentElement; a && a !== document.body; a = a.parentElement) {
				if (/auto|scroll|hidden|clip/.test(getComputedStyle(a).overflowX)) return true;
			}
			return false;
		};
		const culprits = [...document.querySelectorAll('body *')]
			.filter((el) => el.getBoundingClientRect().right > limit + 1 && !scrollsInside(el))
			.slice(0, 8)
			.map((el) => {
				const r = el.getBoundingClientRect();
				return `<${el.tagName.toLowerCase()} class="${el.className}"> right=${Math.round(r.right)}`;
			});
		return { overflow, culprits };
	});
	expect(
		report.overflow,
		`the page must not scroll sideways; past the edge: ${report.culprits.join(', ') || '(nothing found)'}`
	).toBe(0);
}

export const test = base.extend<{ fitsTheScreen: void }>({
	fitsTheScreen: [
		async ({ page }, use) => {
			await use();
			await expectNoSidewaysScroll(page);
		},
		{ auto: true }
	]
});

export { expect };
