import { mountApp } from './ui/app';
import { detectCapabilities, type Capabilities } from './capabilities';

async function bootstrap() {
  const caps = await detectCapabilities();
  logCapabilities(caps);
  mountApp(document.getElementById('app')!, caps);
}

function logCapabilities(caps: Capabilities) {
  console.info('[moonlight] capabilities', caps);
  if (!caps.crossOriginIsolated) {
    console.warn(
      '[moonlight] page is not cross-origin isolated; WASM threads disabled. ' +
        'Ensure COOP/COEP headers are set (see vite.config.ts).',
    );
  }
  if (!caps.webCodecs) {
    console.error('[moonlight] WebCodecs not available - cannot decode video.');
  }
  if (!caps.keyboardLock) {
    console.warn('[moonlight] Keyboard Lock API unavailable - system keys will leak to the OS.');
  }
}

bootstrap().catch((err) => {
  console.error('[moonlight] bootstrap failed', err);
  document.body.textContent = `Moonlight failed to start: ${err.message ?? err}`;
});
