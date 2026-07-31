import { defineConfig } from '@playwright/test';

export default defineConfig({
	testDir: 'e2e',
	// One shared server and corpus: specs run one at a time.
	workers: 1,
	use: {
		baseURL: 'http://127.0.0.1:7940',
		// The primary screen is a phone in a kitchen, so the whole suite
		// runs at that shape — desktop only relaxes the layout. Specs
		// mutate the shared corpus, so they can't simply run twice in a
		// second desktop-sized project.
		viewport: { width: 375, height: 700 }
	},
	webServer: {
		command: 'npm run build && node e2e/serve.mjs',
		url: 'http://127.0.0.1:7940/health',
		reuseExistingServer: false,
		timeout: 240_000
	}
});
