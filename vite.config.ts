import { defineConfig } from 'vite';
import { VitePWA } from 'vite-plugin-pwa';

// SharedArrayBuffer + Emscripten pthreads require cross-origin isolation.
// These headers are mandatory for the WASM module to use threads.
const crossOriginIsolationHeaders = {
  'Cross-Origin-Opener-Policy': 'same-origin',
  'Cross-Origin-Embedder-Policy': 'require-corp',
  'Cross-Origin-Resource-Policy': 'same-origin',
};

export default defineConfig({
  server: {
    host: true,
    port: 5173,
    headers: crossOriginIsolationHeaders,
  },
  preview: {
    headers: crossOriginIsolationHeaders,
  },
  worker: {
    format: 'es',
  },
  build: {
    target: 'es2022',
    sourcemap: true,
  },
  // The wasm module ships in /public/wasm/ and is dynamically imported by
  // the worker at runtime; vite must not try to pre-bundle it.
  assetsInclude: ['**/*.wasm'],
  plugins: [
    VitePWA({
      // Don't auto-register a service worker. With self-signed certs on
      // ChromeOS, the SW fetch fails with "An unknown error occurred when
      // fetching the script" even after the cert is trusted in the cert
      // manager. We can re-enable this for production (real cert) by
      // flipping `injectRegister` back to 'auto'.
      registerType: 'autoUpdate',
      injectRegister: false,
      manifest: false,
      workbox: {
        globPatterns: ['**/*.{js,css,html,ico,png,svg,wasm,webmanifest}'],
        globIgnores: ['**/wasm/moonlight.wasm'],
        navigateFallback: '/index.html',
        navigateFallbackDenylist: [/^\/api\//, /^\/proxy\//],
      },
      devOptions: { enabled: false },
    }),
  ],
});
