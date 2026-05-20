# Moonlight PWA - architecture & handoff doc

This doc is intended as a takeover brief. It explains the design, what is
in place, what is intentionally stubbed, and what to tackle next. Pair it
with the inline `TODO` and `CURRENT STATUS` comments scattered through the
codebase.

---

## 1. The fundamental constraint

Browsers cannot open raw UDP sockets. GameStream and Sunshine speak UDP for
the video, audio, and control streams. **A pure-PWA-to-host topology is
not possible today.** Three workable options exist:

| Option | Pro | Con |
| --- | --- | --- |
| **Host-side proxy** *(chosen)* | Works with every existing server. | Requires installing a small bridge on the host. |
| Sunshine WebRTC datachannel | No companion install. | Sunshine-specific; relies on a still-experimental branch. |
| Isolated Web App + Direct Sockets | Real UDP from the browser. | ChromeOS/Chrome only, install behind enterprise flag. |

We went with **host-side proxy** for breadth of compatibility. The PWA
talks to it over WebTransport (preferred) or WebSocket. The proxy is
documented but **not yet implemented**; see `host-proxy/README.md`.

---

## 2. Process / thread topology

```
+--- Browser tab ----------------------------------------------------+
|                                                                    |
|  Main thread                                                       |
|    - UI / DOM                                                      |
|    - WebCodecs VideoDecoder + canvas renderer (zero-copy bitmap)   |
|    - AudioContext + AudioWorklet (Opus decode + playback)          |
|    - Input capture: Keyboard Lock, Pointer Lock, Gamepad API       |
|    - Posts commands to worker, receives encoded chunks from worker |
|                                                                    |
|  Dedicated worker (src/wasm/worker.ts)                             |
|    - Hosts the WASM module (moonlight-common-c)                    |
|    - Holds the WebTransport / WebSocket session to the proxy       |
|    - moonlight-common-c spawns its own pthreads inside the WASM    |
|      heap (Emscripten -pthread + SharedArrayBuffer)                |
+--- Host-side proxy -----------------------------------------------+
|    - Multiplexed WT/WS server                                     |
|    - Per-channel UDP/TCP socket toward the gaming host            |
+--- Gaming host (Sunshine / GeForce Experience) -------------------+
```

**Why the worker?** Keeps the main thread free for WebCodecs callbacks and
input handling. WebCodecs frame output is delivered on whatever thread
ran `decode()`; we deliberately run the decoder on the main thread so the
canvas update path is one hop instead of two.

**Why a single worker, not one per stream?** Only one stream is active at
a time, and the wasm module owns global state.

---

## 3. Per-component status

### 3.1 PWA shell - **DONE**
- `index.html`, `manifest.webmanifest`, icons, service worker (via
  `vite-plugin-pwa`), COOP/COEP headers, capability detection.
- Visible at `npm run dev`.

### 3.2 WASM build - **scaffolded, untested**
- `wasm/CMakeLists.txt` lists all `moonlight-common-c` sources plus enet
  and nanors. Link flags enable `-pthread`, `MODULARIZE`, `ASYNCIFY`, and
  export the `mlw_*` C entry points.
- `wasm/src/bindings.c` registers the renderer + listener callbacks and
  forwards Annex-B NALUs and Opus frames to JS.
- `wasm/src/platform_web.c` defines a channel table and stubs the bridge.
- `wasm/moonlight_imports.js` is the `--js-library` that links the C-side
  `extern` declarations to JS implementations.

**Not yet done:**
- `--js-library=wasm/moonlight_imports.js` is referenced in this doc but
  not added to `LINK_FLAGS` in CMakeLists. Add it once `build.sh` runs.
- `platform_web.c` does **not** actually intercept moonlight-common-c's
  socket calls. The recommended approach is to upstream a small
  `#ifdef __EMSCRIPTEN__` patch into `PlatformSockets.c` that calls
  `mlw_channel_*` instead of `socket()/bind()/recvfrom()/sendto()`. The
  alternative - hooking via dlsym + `--js-library` overriding libc - is
  brittle.
- `build.sh` has never been run end-to-end. First test: install
  `emsdk` 3.1.50+, `source emsdk_env.sh`, then `npm run wasm`.

### 3.3 Video pipeline - **mostly done**
- `WebCodecsVideoRenderer` configures a `VideoDecoder` with
  `optimizeForLatency: true` and `hardwareAcceleration: 'prefer-hardware'`.
- Renders via `ImageBitmapRenderingContext.transferFromImageBitmap` when
  available, falling back to `CanvasRenderingContext2D.drawImage`.
- Tracks decode latency, FPS, and dropped frames.

**Not yet done:**
- No back-channel from the renderer to request an IDR when decode throws.
  Wire `decoder.error` -> `client.requestIdr()`.
- HDR metadata is ignored (`SS_HDR_METADATA`). Hook
  `LiGetHdrMetadata` -> `VideoFrame.colorSpace` once we light up HEVC HDR.
- No render-on-vsync. The current path renders as fast as `createImageBitmap`
  resolves; for tear-free output, switch to `requestVideoFrameCallback` on
  an offscreen `<video>` element fed by a `MediaStreamTrackGenerator`.

### 3.4 Audio pipeline - **scaffolded**
- `AudioRenderer` creates a 48 kHz `AudioContext` and an
  `AudioWorkletNode` (`opus-worklet.js`).
- Bootstraps from a 16-byte config block emitted by the wasm-side
  `audInit`.

**Not yet done:**
- The worklet currently **plays silence** because Opus decoding is
  stubbed. Two paths:
  - Use the WebCodecs `AudioDecoder` API on the main thread, ship PCM
    chunks to the worklet. Cleanest but availability is uneven as of
    early 2026.
  - Ship `libopus` as WASM into the worklet (the worklet doesn't load
    modules cleanly; use a single-file build like `libopusjs`).
- Surround / 5.1 / 7.1 support. `audioConfiguration` is hard-wired to
  stereo in `worker.ts`.

### 3.5 Input - **mostly done**
- **Keyboard:** uses `navigator.keyboard.lock(['Escape', 'Tab', 'Meta*',
  'Alt*', 'F11', 'PrintScreen', 'ContextMenu'])`. Requires fullscreen +
  user activation, which `StreamView.start()` arranges. VK code mapping
  follows the NaCl client's mapping with left/right modifier separation.
  Ctrl+Alt+Shift+Q tears down the stream like the original client.
- **Pointer:** Pointer Lock on the canvas, accumulates deltas, flushes
  every 5 ms. Maps wheel ticks to moonlight's 120-per-tick high-res scroll.
- **Gamepad:** Polls `navigator.getGamepads()` on RAF, mirrors the
  upstream button bitmap, sends sticks as int16.

**Not yet done:**
- Rumble feedback. The wasm callback `mlw_js_rumble` is plumbed but the
  worker doesn't forward to the main thread, and the main thread doesn't
  call `gamepad.vibrationActuator.playEffect`.
- Touch input. The original NaCl client supports both "native touch
  events" and emulated mouse. Wire `PointerInput` to handle PointerEvents
  of type `touch` and dispatch via `LiSendTouchEvent`.
- Adaptive triggers / motion / LED for DualSense. Forwarded over the wire
  but no PWA UI yet.

### 3.6 Pairing & NvHTTP - **stubbed**
- `nvhttp.ts` has the request shapes for `/serverinfo`, `/applist`,
  `/launch`.
- `pairing.ts` generates an RSA keypair via WebCrypto but does **not**
  produce a valid X.509 cert and does **not** execute the five-step
  pairing handshake.

**Path forward:** push pairing + NvHTTP into the host proxy. Expose
`/api/nvhttp` (forwards plus injects the client cert) and `/api/pair`
(drives the handshake) on the proxy. This sidesteps Private Network
Access, CORS, and the WebCrypto-can't-mint-certs problem in one go.

### 3.7 Host proxy - **NOT IMPLEMENTED**
- Documented in `host-proxy/README.md`.
- Recommended stack: Rust + tokio + quinn (WebTransport) + tungstenite
  (WebSocket fallback). Plus a small XML/HTTP forwarder for NvHTTP.

---

## 4. End-to-end demo path (when finished)

1. User opens the PWA, installs it.
2. User adds a host by IP.
3. PWA hits the proxy's `/api/pair` to perform the five-step handshake
   with the host. Proxy stores the resulting cert pair; PWA stores a
   reference token in localStorage.
4. User picks an app to launch. PWA hits proxy's `/api/launch` -> proxy
   calls NvHTTP `/launch` -> returns RTSP session URL.
5. PWA `MoonlightClient.connect()` posts `start` to the worker.
6. Worker loads the WASM module, opens the WT session to the proxy.
7. `mlw_start` calls `LiStartConnection`. moonlight-common-c opens
   virtual sockets via `mlw_channel_*`; the proxy bridges them to real
   UDP / TCP.
8. Video NALUs land in `mlw_js_video_submit` -> worker `postMessage`s
   them to the main thread -> `WebCodecsVideoRenderer.submit()`.
9. Opus frames take the same path through `AudioRenderer`.
10. Keyboard / pointer / gamepad events post commands to the worker,
    which calls `mlw_send_*`.

---

## 5. Build / dev tasks

* `npm run dev` - vite dev server with COOP/COEP set.
* `npm run wasm` - emscripten build of moonlight-common-c.
* `npm run build` - full PWA build to `dist/`.
* `npm run typecheck` - TypeScript check.

The COOP/COEP headers are critical for SharedArrayBuffer + threaded WASM.
Production deployments must replicate them; see `vite.config.ts` for the
exact header list.

---

## 6. Outstanding work, ranked by blocking-ness

1. **Host proxy.** Nothing else can connect until this exists.
2. **PlatformSockets shim.** Wire `mlw_channel_*` into upstream
   `PlatformSockets.c` under `#ifdef __EMSCRIPTEN__`. Get a successful
   `LiStartConnection` against a mocked proxy that just echoes.
3. **Opus decoding inside the worklet.** Otherwise audio is silent.
4. **Pairing.** Either move it into the proxy (recommended) or finish
   the WebCrypto-based implementation in `pairing.ts`.
5. **HDR + HEVC selection logic.** WebCodecs supports both; the host
   capabilities query needs to flow through the codec picker in
   `StreamView`.
6. **Touch + rumble + DualSense extras.**

---

## 7. Known browser caveats

* **Safari:** Keyboard Lock API is not implemented as of early 2026.
  Esc / Alt+Tab will leak to macOS. WebCodecs is supported since 16.4.
* **Firefox:** Keyboard Lock shipped in 130. WebTransport is still
  behind a pref; we will hit the WebSocket fallback there.
* **Cross-origin isolation:** the page must be served with both
  `Cross-Origin-Opener-Policy: same-origin` and
  `Cross-Origin-Embedder-Policy: require-corp`. Without these,
  SharedArrayBuffer is disabled and the WASM module won't run.
* **Private Network Access:** browsers block fetches from public-origin
  PWAs to RFC1918 IPs unless the target opts in via CORS preflight with
  `Access-Control-Allow-Private-Network: true`. Sunshine doesn't, GFE
  doesn't. This is the single biggest reason NvHTTP must go through the
  proxy.
