// A pure SPA: nothing prerenders, nothing server-renders; the static
// adapter emits index.html as a fallback and the app routes client-side.
export const ssr = false;
export const prerender = false;
