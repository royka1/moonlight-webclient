// Pairing is driven by the host proxy.
//
// The browser cannot:
//   - mint a self-signed X.509 cert cleanly (WebCrypto can do the keys
//     but not ASN.1 cert construction without shipping a big lib)
//   - reach a Sunshine/GFE host on an RFC1918 address without fighting
//     Private Network Access + the host's self-signed TLS cert
//
// So we hand both jobs to the proxy. The PWA generates a PIN, posts it to
// `POST /api/pair`, the proxy runs the five-step NvHTTP handshake, then
// returns the resulting cert pair which we cache against the host record.
//
// Proxy contract (request):
//   POST /api/pair
//   Content-Type: application/json
//   { "address": "192.168.1.42", "pin": "1234" }
//
// Proxy contract (response, success):
//   { "paired": true,
//     "serverCert": "-----BEGIN CERTIFICATE-----\n...",
//     "clientCert": "-----BEGIN CERTIFICATE-----\n...",
//     "clientKey":  "-----BEGIN PRIVATE KEY-----\n..." }
//
// Proxy contract (response, failure):
//   { "paired": false, "error": "human-readable reason" }
//
// The proxy SHOULD stream progress messages back via Server-Sent Events on
// the same connection (one event per pairing step). The PWA listens with
// EventSource semantics: lines beginning with `data:` are JSON events of
// the form `{"step":N,"total":5,"message":"..."}`. When the SSE stream
// closes with a final `{"paired":true|false,...}` event, the handshake is
// considered terminal.

import type { Host } from './host-store';
import { getDeviceName } from './settings';

export interface PairProgress {
  step: number;
  total: number;
  message: string;
}

export interface PairOptions {
  signal?: AbortSignal;
  onProgress?: (p: PairProgress) => void;
  /** Override the proxy origin. Defaults to window.location.origin. */
  proxyOrigin?: string;
}

interface PairResponse {
  paired: boolean;
  serverCert?: string;
  clientCert?: string;
  clientKey?: string;
  error?: string;
}

const TOTAL_STEPS = 5;

export async function pairHost(host: Host, pin: string, opts: PairOptions = {}): Promise<Host> {
  const origin = opts.proxyOrigin ?? window.location.origin;
  const url = `${origin}/api/pair`;

  opts.onProgress?.({ step: 0, total: TOTAL_STEPS, message: 'Contacting host…' });

  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Accept: 'application/x-ndjson, application/json' },
    body: JSON.stringify({
      address: host.address,
      pin,
      deviceName: getDeviceName(),
    }),
    signal: opts.signal,
  });

  if (!res.ok) {
    throw new Error(`proxy /api/pair returned ${res.status}: ${await safeText(res)}`);
  }

  // Stream NDJSON / SSE-style progress events if the proxy supports it.
  // Fall back to a single JSON body if it doesn't.
  const ct = res.headers.get('content-type') ?? '';
  const result: PairResponse = ct.includes('json') && res.body
    ? await consumeStream(res.body, opts.onProgress)
    : (await res.json()) as PairResponse;

  if (!result.paired) {
    throw new Error(result.error ?? 'host rejected the pairing PIN');
  }

  return {
    ...host,
    paired: true,
    serverCert: result.serverCert,
    clientCert: result.clientCert,
    clientKey: result.clientKey,
    lastSeen: Date.now(),
  };
}

async function consumeStream(
  body: ReadableStream<Uint8Array>,
  onProgress?: (p: PairProgress) => void,
): Promise<PairResponse> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buf = '';
  let last: PairResponse = { paired: false };

  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    // NDJSON: one JSON object per line.
    let nl: number;
    while ((nl = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, nl).trim();
      buf = buf.slice(nl + 1);
      if (!line) continue;
      try {
        const ev = JSON.parse(line);
        if (typeof ev.step === 'number') {
          onProgress?.({
            step: ev.step,
            total: ev.total ?? TOTAL_STEPS,
            message: ev.message ?? `Step ${ev.step}/${ev.total ?? TOTAL_STEPS}`,
          });
        }
        if (typeof ev.paired === 'boolean') {
          last = ev as PairResponse;
        }
      } catch {
        // Ignore malformed lines; the final body is what counts.
      }
    }
  }
  // Tail: a final partial line without a trailing newline.
  if (buf.trim()) {
    try { last = JSON.parse(buf.trim()) as PairResponse; } catch { /* noop */ }
  }
  return last;
}

async function safeText(res: Response): Promise<string> {
  try { return (await res.text()).slice(0, 200); } catch { return ''; }
}
