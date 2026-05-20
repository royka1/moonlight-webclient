# Moonlight PWA

Low-latency NVIDIA GameStream / [Sunshine](https://github.com/LizardByte/Sunshine)
client that runs in any Chromium-based browser. Built specifically for
ChromeOS / FydeOS, where the original `moonlight-chrome` NaCl client no
longer works — but it also runs on Windows, Linux and macOS Chrome.

- **Decode latency around 2 ms** with hardware H.264 (SPS surgery convinces
  Chrome's decoder to stop buffering reference frames).
- **MediaStreamTrackGenerator → `<video>`** rendering path so ChromeOS puts
  the stream on a hardware overlay plane and skips the GPU compositor.
- **In-worker decoder**: the wasm worker that runs moonlight-common-c also
  owns the `VideoDecoder` and writes decoded frames straight into the
  MediaStreamTrack — no chunk data crosses worker↔main.
- **Single binary, embedded PWA** — no separate install steps, no
  `--www-root` to remember.
- **Pair, launch, control** flows match moonlight-android / moonlight-qt.
  Touchpad, touchscreen, mouse, keyboard (with Keyboard Lock), gamepad.

> Status: working end-to-end on Sunshine. The protocol implementation
> is straight moonlight-common-c, so anything that talks to that should
> work; the proxy uses NvHTTP for pairing and `/launch` and bridges the
> RTSP/Enet/RTP traffic verbatim.

---

## Quick start — just want to stream

1. Grab the latest archive for your OS from
   [Releases](../../releases):

   | OS | Asset |
   |----|-------|
   | Windows | `moonlight-pwa-windows-x86_64.zip` |
   | Linux x86_64 | `moonlight-pwa-linux-x86_64.tar.gz` |
   | macOS (Apple Silicon) | `moonlight-pwa-macos-aarch64.tar.gz` |

2. Extract anywhere. Run the binary:

   ```bash
   # Windows
   moonlight-host-proxy.exe

   # Linux / macOS
   ./moonlight-host-proxy
   ```

   First run generates a self-signed TLS cert and pairing identity under
   the platform's app-data directory (`%LOCALAPPDATA%\moonlight\proxy` on
   Windows, `~/.local/share/moonlight/proxy` on Linux, etc.).

3. Open `https://localhost:47999` in Chrome / Edge / FydeOS browser. You'll
   need to trust the self-signed cert once — the URL bar shows you the
   warning, click "Advanced → Proceed to localhost".

4. Click **Add host**, type the LAN IP of your Sunshine machine, then
   click the host card to pair. Sunshine pops a PIN dialog on the host's
   web UI; type the 4-digit PIN the PWA shows you and submit. Pairing
   completes automatically.

5. Click the host again to pick an app and stream.

### Install as a real app

In the URL bar there's an install button (the little screen-with-arrow
icon). Install once and you get a fullscreen, chromeless launcher that
takes a vsync less than running in a tab.

---

## Quick start — build from source

You need `node >= 22`, `cargo` (stable Rust), `cmake >= 3.20`, `git`, and
[Emscripten](https://emscripten.org/docs/getting_started/downloads.html)
activated in your shell (`source /path/to/emsdk/emsdk_env.sh`). On
Windows, also `nasm` and `strawberryperl` (`choco install -y nasm
strawberryperl`).

```bash
# Clone the repo with the moonlight-common-c submodule.
git clone --recursive https://github.com/royka1/moonlight-webclient
cd moonlight-webclient

# Build the PWA + wasm.
npm ci
bash wasm/build.sh
npm run build

# Build the proxy with the PWA embedded into the binary.
cd host-proxy
cargo build --release
./target/release/moonlight-host-proxy            # linux/macos
# .\target\release\moonlight-host-proxy.exe      # windows
```

Iterating on the PWA without recompiling the proxy:

```bash
# Terminal 1 — Vite dev server with HMR.
npm run dev          # https://localhost:5173

# Terminal 2 — proxy pointing at your live dist/ instead of the embedded one.
cargo run --release -- --www-root ../dist
```

---

## Architecture

```
+---------------------------------------------------+
|  Browser tab — main thread                        |
|    Pair dialog, settings, host picker             |
|    <video>  ←  MediaStreamTrackGenerator          |
+---------------------------------------------------+
                       ▲ writable stream (transferred to worker)
                       │
+---------------------------------------------------+
|  Wasm worker                                      |
|   moonlight-common-c (Emscripten + pthreads)      |
|     ├── RTSP / control / FEC / depacketizer       |
|     └── audio + video chunks                      |
|   VideoDecoder ── output → MSTG writer            |
|   AudioRenderer message → main                    |
+---------------------------------------------------+
                       ▲ WebSocket (binary frames)
                       │  one frame per UDP datagram
+---------------------------------------------------+
|  Host proxy (Rust / axum / tokio / rustls)        |
|   HTTPS server  →  PWA static files (embedded)    |
|   /api/pair     →  NvHTTP 5-step PIN handshake    |
|   /api/launch   →  POST /launch + RTSP url        |
|   /proxy (ws)   →  per-session UDP/TCP relays     |
+---------------------------------------------------+
                       ▲ UDP/TCP on LAN
                       │
+---------------------------------------------------+
|  Sunshine / GFE host                              |
+---------------------------------------------------+
```

The browser cannot speak raw UDP and cannot reach a Sunshine host on an
RFC1918 address without fighting Private Network Access and the host's
self-signed TLS cert. The Rust proxy solves both: it terminates the PWA's
TLS to give browser features that need a secure context (WebCodecs,
SharedArrayBuffer, Keyboard Lock, pointer-lock options), and forwards
each multiplexed channel to the host as the right wire protocol.

### Key files

| Path | Purpose |
| ---- | ------- |
| `host-proxy/src/main.rs` | Server entry point, CLI flags, TLS bring-up. |
| `host-proxy/src/http.rs` | axum router + the embedded-PWA fallback. |
| `host-proxy/src/pairing.rs` | NvHTTP 5-step pairing handshake. |
| `host-proxy/src/nvhttp.rs` | `/serverinfo`, `/applist`, `/launch` proxy. |
| `host-proxy/src/ws_session.rs` | Per-connection channel multiplexer. |
| `host-proxy/src/udp_relay.rs` | UDP relay (Sunshine RTP / control / audio). |
| `host-proxy/src/tcp_relay.rs` | TCP relay (RTSP). |
| `src/wasm/worker.ts` | Wasm worker host + in-worker VideoDecoder + AVCC surgery. |
| `wasm/src/bindings.c` | C glue between moonlight-common-c and JS sinks. |
| `wasm/src/platform_web.c` | PlatformSockets.c replacement (channel multiplexer). |
| `src/video/webcodecs-decoder.ts` | Surface holder (`<video>` + MSTG). |
| `src/input/pointer.ts` | Mouse / touchpad / touchscreen / pen routing. |

---

## Configuration

Click the **⚙ Settings** button in the host list:

| Setting | Notes |
|---------|-------|
| **Resolution** | 720p / 1080p / 1440p / 4K, or **Custom…** with width × height inputs |
| **Frame rate** | 30 / 60 / 90 / 120 fps |
| **Codec** | H.264 (most compatible), HEVC, AV1. AV1 / HEVC need hardware support on both ends. |
| **Audio** | Stereo / 5.1 / 7.1 — must also be enabled on the host. |
| **Bitrate** | Mbps. The **Auto** button recomputes the moonlight-android default for the chosen resolution + fps. |
| **Statistics** | Toggle the on-stream FPS / latency / dropped-frame overlay. |

Settings persist to `localStorage` and take effect on the next launch.

### Proxy CLI flags

```
moonlight-host-proxy [--bind 0.0.0.0] [--port 47999] [--www-root <PATH>]
                     [--data-dir <PATH>] [--mdns] [--log-level info]
                     [--max-channels 64]
```

`--www-root` overrides the embedded PWA bundle (handy for development).
`--data-dir` is where the pairing identity and TLS cert live.

---

## Why this is fast

ChromeOS adds 1–2 vsyncs of compositor latency over native (Android
MediaCodec) clients no matter what you do. The PWA fights for every
millisecond elsewhere:

1. **MediaStreamTrackGenerator → `<video>`** instead of canvas `drawImage`.
   The `<video>` element is promoted to a hardware overlay plane, which
   bypasses the GPU compositor for the actual frame pixels.
2. **In-worker VideoDecoder.** The MSTG writable side is transferred from
   the main thread to the wasm worker; chunk data never crosses
   worker↔main. Saves the postMessage round trip per frame.
3. **AVCC description with patched SPS.** Sunshine emits an H.264 SPS with
   no `bitstream_restriction_flag` set, so Chrome's decoder conservatively
   buffers ~4 frames assuming there might be B-frames. We parse the SPS
   bit-by-bit, inject `bitstream_restriction_flag=1` +
   `max_num_reorder_frames=0`, build a fresh AVCC extradata, and
   reconfigure the decoder on the first IDR. Decode latency goes from
   60–130 ms to 2 ms.
4. **Resolution-correct codec strings.** `avc1.640028` (level 4.0) max is
   1080p30; the worker picks the right level for the actual resolution +
   fps so Chrome doesn't silently route to software decode.
5. **Win-only UDP fix.** `WSAIoctl(SIO_UDP_CONNRESET, FALSE)` in the proxy
   stops ICMP-unreachable replies from killing the audio/video recv loops.
6. **Stats overlay auto-hides** so a transparent sibling element doesn't
   demote the video out of the hardware overlay.

---

## Known limitations / things to expect

- **Decoder buffering on browsers without VUI hint support.** Chromium ≥
  124 honours the patched SPS. Older browsers still buffer.
- **Touchpad tap under pointer lock** can be filtered by ChromeOS at the
  OS level on certain touchpad firmware (newer haptic touchpads in
  particular). Physical click always works. There's no JS-side fix when
  the OS swallows the event.
- **Touchscreen multi-touch.** Moonlight has no multi-touch protocol;
  secondary fingers are ignored.
- **WebTransport.** Code paths exist but the default transport is
  WebSocket. WT was deprioritised in favour of getting the WS path
  rock-solid.
- **HDR / 10-bit.** Not exposed in the UI; the decoder paths are in
  place but untested.

---

## Contributing

PRs welcome. CI runs `tsc --noEmit`, `clippy -D warnings`, and the full
proxy build on every push.

To debug latency / decode behaviour, watch the worker console for the
periodic `[video] submit=N decoded=M qNow=Q qMax=Q' gap=G.Gms lat=L.Lms`
line. `gap` ≈ real per-frame decode work, `lat` includes any internal
buffering. If those diverge, something upstream is queuing.

---

## License

GPL-2.0-or-later, inherited from
[moonlight-common-c](https://github.com/moonlight-stream/moonlight-common-c)
which is linked into the wasm binary.

The original NaCl client (`moonlight-chrome`) was also GPL.
