import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
	plugins: [sveltekit()],
	server: {
		// Dev runs against a local mise-server.
		proxy: {
			'/api': 'http://127.0.0.1:7920',
			'/chat': 'http://127.0.0.1:7920',
			'/health': 'http://127.0.0.1:7920'
		}
	},
	test: {
		include: ['src/**/*.test.ts'],
		environment: 'node'
	}
});
