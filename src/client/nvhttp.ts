// Talk to NvHTTPS via the host-side proxy (HTTPS:47984 with mTLS), not
// directly to Sunshine - browsers can't attach a client cert to fetch()
// and Private Network Access blocks RFC1918 calls from a public origin.
//
// Proxy endpoints (see host-proxy/src/nvhttp.rs):
//   GET  /api/applist?host=<addr>
//   POST /api/launch     { host, appId, width, height, fps, bitrate,
//                          audioConfig, riKeyHex, riKeyId, resume? }

import type { Host } from './host-store';
import type { StreamConfig } from './moonlight-client';

export interface AppEntry {
  id: number;
  title: string;
  hdrSupported?: boolean;
}

export async function fetchAppList(host: Host): Promise<AppEntry[]> {
  const url = new URL('/api/applist', window.location.origin);
  url.searchParams.set('host', host.address);
  const res = await fetch(url, { headers: { Accept: 'application/json' } });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    throw new Error(err.error ?? `applist failed: ${res.status}`);
  }
  const data = (await res.json()) as { apps: AppEntry[] };
  return data.apps;
}

export interface LaunchResult {
  rtspSessionUrl: string;
  gameSession: string;
  appVersion: string;
  gfeVersion: string;
  /** Hex of the 16-byte RI key we generated and sent. The wasm worker
   *  needs this exact value when calling LiStartConnection. */
  riKeyHex: string;
  riKeyId: number;
}

export interface LaunchOptions {
  host: Host;
  app: AppEntry;
  config: StreamConfig;
  resume?: boolean;
}

export async function launchApp(opts: LaunchOptions): Promise<LaunchResult> {
  // Generate a fresh remote-input AES key per session.
  const riKeyBytes = crypto.getRandomValues(new Uint8Array(16));
  const riKeyHex = Array.from(riKeyBytes, (b) => b.toString(16).padStart(2, '0')).join('');
  // Random positive int32.
  const riKeyId = Math.floor(Math.random() * 0x7fffffff);

  const url = new URL('/api/launch', window.location.origin);
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
    body: JSON.stringify({
      host: opts.host.address,
      appId: opts.app.id,
      width: opts.config.width,
      height: opts.config.height,
      fps: opts.config.fps,
      bitrate: opts.config.bitrateKbps,
      audioConfig: opts.config.audioConfiguration,
      riKeyHex,
      riKeyId,
      resume: opts.resume ?? false,
    }),
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    throw new Error(err.error ?? `launch failed: ${res.status}`);
  }
  const data = (await res.json()) as Omit<LaunchResult, 'riKeyHex' | 'riKeyId'>;
  return { ...data, riKeyHex, riKeyId };
}
