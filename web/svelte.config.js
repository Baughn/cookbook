import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	preprocess: vitePreprocess(),
	kit: {
		// A pure SPA: the server knows nothing about routes, unknown paths
		// fall back to index.html and the app routes client-side.
		adapter: adapter({ fallback: 'index.html' })
	}
};

export default config;
