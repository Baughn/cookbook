import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	preprocess: vitePreprocess(),
	kit: {
		// A pure SPA: the server knows nothing about routes, unknown paths
		// fall back to index.html and the app routes client-side.
		adapter: adapter({ fallback: 'index.html' }),
		// The CSP rides in the page as a meta tag — the server can't know
		// this build's inline-bootstrap hash, so the build carries its own
		// policy. Header-only directives (frame-ancestors) come from
		// mise-server's static-response headers instead.
		csp: {
			mode: 'hash',
			directives: {
				'default-src': ['self'],
				'script-src': ['self'],
				// Svelte sets style *attributes* (transitions, the app
				// shell's display:contents); attributes can't run script.
				'style-src': ['self', 'unsafe-inline'],
				// data:/blob: so photo previews never fight the policy;
				// image data URIs don't execute.
				'img-src': ['self', 'data:', 'blob:'],
				'connect-src': ['self'],
				'object-src': ['none'],
				'base-uri': ['self']
			}
		}
	}
};

export default config;
