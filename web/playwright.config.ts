import { defineConfig } from '@playwright/test';

export default defineConfig({
	testDir: 'e2e',
	// One shared server and corpus: specs run one at a time.
	workers: 1,
	use: { baseURL: 'http://127.0.0.1:7940' },
	webServer: {
		command: 'npm run build && node e2e/serve.mjs',
		url: 'http://127.0.0.1:7940/health',
		reuseExistingServer: false,
		timeout: 240_000
	}
});
