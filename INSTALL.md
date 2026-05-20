# Installing Moonlight PWA on ChromeOS / FydeOS

This guide walks through getting the PWA installed on a Chromebook (or any
ChromiumOS-based device, including FydeOS) and connected to a Sunshine /
GeForce Experience host on your LAN.

If you've never built the project, do that first:

```sh
npm install
npm run wasm     # one-time, fetches & builds mbedtls
npm run build    # produces dist/
```

The end state of those commands is a `dist/` directory holding a fully
static PWA. The rest of this doc is about how to serve and install it.

---

## 1. Serve the PWA from somewhere HTTPS-reachable

A PWA needs three things to install and run threaded WASM:

| Requirement | Why |
| --- | --- |
| HTTPS (or `localhost`) | service-worker + secure-context APIs |
| `Cross-Origin-Opener-Policy: same-origin` | enables `SharedArrayBuffer` |
| `Cross-Origin-Embedder-Policy: require-corp` | same |

The path of least resistance is to **let the host proxy serve `dist/` as
static files on the same TLS endpoint** it already exposes for the
WebTransport / WebSocket bridge. Single origin = no CORS pain and the PWA
can hit the proxy with a same-origin `wss://...` URL.

### If your host proxy is Rust + axum

```rust
use axum::{routing::get, Router};
use http::{header, HeaderValue};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

let app = Router::new()
    .route("/proxy", get(ws_handler))         // your existing handler
    .route("/api/nvhttp", get(nvhttp_proxy))  // (when wired up)
    .fallback_service(ServeDir::new("dist"))
    .layer(SetResponseHeaderLayer::overriding(
        header::HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    ))
    .layer(SetResponseHeaderLayer::overriding(
        header::HeaderName::from_static("cross-origin-embedder-policy"),
        HeaderValue::from_static("require-corp"),
    ));
```

### If your host proxy is Go + chi/echo

```go
r.Use(middleware.SetHeader("Cross-Origin-Opener-Policy", "same-origin"))
r.Use(middleware.SetHeader("Cross-Origin-Embedder-Policy", "require-corp"))
r.Handle("/proxy", wsHandler)
r.Handle("/*", http.FileServer(http.Dir("dist")))
```

### Alternatives if you don't want to bake serving into the proxy

* **Caddy** alongside the proxy:

  ```caddyfile
  https://gamerig.local {
      tls internal
      header Cross-Origin-Opener-Policy   "same-origin"
      header Cross-Origin-Embedder-Policy "require-corp"
      reverse_proxy /proxy   localhost:47999
      file_server / browse  { root /opt/moonlight-pwa/dist }
  }
  ```

* **GitHub Pages / Cloudflare Pages** + a `public/_headers` file:

  ```
  /*
    Cross-Origin-Opener-Policy: same-origin
    Cross-Origin-Embedder-Policy: require-corp
  ```

  You then talk to the home gaming PC via Cloudflare Tunnel / Tailscale
  Funnel / a DDNS hostname.

---

## 2. Trust the proxy's TLS cert on the Chromebook

The PWA loads over HTTPS, so it can only open `wss://` — which means the
proxy needs a cert ChromeOS will trust. Three options, ranked by how
painful they are:

### Option A - Real cert (best UX)

Get a public hostname (your own domain, or **Tailscale MagicDNS** which
hands out `*.ts.net` for free) and run certbot with the DNS-01 challenge.
Now every device trusts the cert with zero per-device setup.

```sh
sudo certbot certonly --dns-cloudflare \
  -d gamerig.example.com \
  --preferred-challenges dns-01
```

Tailscale users can let Tailscale terminate TLS itself with `tailscale serve`:

```sh
tailscale serve --https=443 /proxy proxy://localhost:47999
tailscale serve --https=443 /        path:/opt/moonlight-pwa/dist
```

### Option B - Self-signed cert, manually trusted

1. On the gaming host, export your proxy's CA certificate as PEM.
   (If the proxy generated a single self-signed leaf, that's also fine -
   treat it as the CA.)
2. Copy the `.pem` to the Chromebook (USB, email, GDrive, whatever).
3. ChromeOS / FydeOS: open `chrome://certificate-manager`
   → **Custom** → **Installed by you** → **Import**.
4. Tick **Trust this certificate for identifying websites**.
5. Verify by visiting `https://<proxy-host>` - no certificate warning.

**Caveats:**
* On enterprise- or school-managed Chromebooks, certificate import is
  sometimes blocked by policy. `chrome://policy` will tell you.
* FydeOS works the same as upstream ChromeOS here.

### Option C - Tunnel that terminates TLS for you

* `cloudflared tunnel --hostname gamerig.example.com --url https://localhost:47999`
* `tailscale serve` (see above)
* `frp` / `ngrok` / `rathole` if you really want.

All three give you a public hostname with a valid cert and a reverse
tunnel to the gaming PC.

---

## 3. Install the PWA on ChromeOS / FydeOS

1. On the Chromebook, open Chrome and navigate to your serving URL
   (e.g. `https://gamerig.example.com` or `https://192.168.1.42:47999`).
2. The page should render the host list. If it doesn't, jump to
   [Troubleshooting](#troubleshooting).
3. Click the **install icon** in the address bar (looks like a monitor
   with a down-arrow). On smaller screens it lives in the **⋮** menu
   under "Install Moonlight…".
4. Confirm. The PWA gets its own launcher entry and opens in a standalone
   window without browser chrome.
5. Right-click the launcher tile → **Pin to shelf** if you want one-tap
   access. **Open as window** is enabled by default for installed PWAs.

To launch fullscreen automatically every time, use the PWA's settings
icon → **Always open in window** → then **F4** to go fullscreen inside
the app (or set the system action on Search+F).

---

## 4. ChromeOS-specific runtime notes

* **Keyboard Lock**: works in CrOS Chrome 76+, but the **Search/Launcher
  key is never lockable** - that's an OS-level guarantee. Esc, Alt+Tab,
  F-keys, Meta combos all get captured once the page is fullscreen and
  the PWA has called `navigator.keyboard.lock()` (we do this in
  `src/input/keyboard.ts`).
* **Fullscreen**: F4 enters/exits fullscreen. The shelf auto-hides. The
  PWA also requests fullscreen programmatically when you click a host
  card (see `StreamView.start()`).
* **System notification on key lock**: ChromeOS shows a banner that says
  "Press and hold Esc to exit". This is OS-controlled, not dismissible
  from the app.
* **Gamepads**:
  * Wired USB / USB-C controllers Just Work.
  * Bluetooth controllers depend on the kernel - Xbox One controllers
    work since CrOS M97; older PS4 needs a USB cable.
* **Touchscreen / tablet mode**: pointer events arrive, but the PWA
  doesn't yet ship a touch input layer (see `PLAN.md §3.5`). Bring a
  mouse for now.
* **External display**: works. Fullscreen the PWA window on the second
  display before connecting; the canvas will scale to the display's
  resolution.

---

## 5. Connecting to your host

Once the PWA is open:

1. Click **+ Add host** and enter the gaming PC's address. If you went
   with Tailscale, use the `*.ts.net` hostname; otherwise the LAN IP.
2. The first time you click a host, the PWA will need to **pair**. The
   pairing flow (`src/client/pairing.ts`) is still stubbed in this build
   - the recommended path is to drive it from the host proxy and surface
   a single `/api/pair?pin=1234` endpoint. See `PLAN.md §3.6`.
3. After pairing succeeds, host cards become clickable to stream.

---

## Troubleshooting

| Symptom | Likely cause / fix |
| --- | --- |
| Address bar has no install icon | Service worker didn't register. Open DevTools → Application → Service Workers. Reload twice. |
| Console: `crossOriginIsolated is false` | Your server isn't sending COOP/COEP headers. Check with `curl -I https://your-host/`. |
| `WebTransport is not defined` | Browser too old, or `WebTransport` flag disabled. Fallback to `ws://` works automatically; check console. |
| `NET::ERR_CERT_AUTHORITY_INVALID` | Cert isn't trusted. Re-do step 2 of this guide. |
| `Mixed Content: ... requested an insecure WebSocket connection` | PWA is on HTTPS but trying to use `ws://`. Pass `proxyUrl: 'wss://...'` (the default) and make sure the proxy serves TLS. |
| Stream window opens but is black | WebCodecs couldn't decode. Open DevTools → Console; the renderer logs decode errors. Most often: codec mismatch (HEVC vs. H.264). The capability probe in `src/capabilities.ts` shows what's hardware-supported. |
| Keyboard lock denied | Either the page isn't fullscreen, or there was no recent user gesture. The PWA enters fullscreen on click, which should satisfy both. |
| Audio is silent | Expected in this build - the Opus decoder inside the AudioWorklet is stubbed. See `PLAN.md §3.4`. |

For anything else, `chrome://inspect` → **Devices** lets you remote-debug
the PWA from your dev machine over USB / network.

---

## Uninstall

* PWA: right-click the launcher tile → **Uninstall**. Removes the PWA
  but leaves the underlying browser data unless you also clear it via
  `chrome://settings/siteData`.
* Cert: `chrome://certificate-manager` → find it → **Delete**.
* The host proxy is a separate install on the gaming PC and removed
  there.
