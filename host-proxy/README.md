# Moonlight host proxy

Browsers can't open raw UDP sockets to a gaming host. The PWA needs a
companion process to bridge between WebSocket / WebTransport (in the
browser) and the UDP / TCP traffic that GameStream and Sunshine speak.

The proxy is **not implemented** yet. This README documents the
expected behaviour so a future contributor can build it.

## Requirements

* Listen on a TLS endpoint (`https://localhost:47999` is the default the
  PWA uses; configurable via `proxyUrl` on `MoonlightClient`). Self-signed
  cert is fine - the user trusts it on first run.
* Prefer **WebTransport over HTTP/3** when the client supports it; fall
  back to a single binary WebSocket otherwise.
* Implement the multiplexing protocol defined in
  `src/transport/proxy-transport.ts`.
* For each opened channel:
  * `proto = 1` (UDP): bind an ephemeral UDP socket, send packets to the
    target host:port, forward inbound packets back over the channel.
  * `proto = 2` (TCP): open a TCP connection to host:port, mirror
    bidirectionally.
* Pass-through HTTP(S) for the NvHTTP control endpoint at
  `/api/nvhttp?url=<target>` so the PWA doesn't have to fight Private
  Network Access / CORS / cert pinning in the browser.

## Suggested implementation

* Language: **Rust** (Tokio + quinn for WebTransport + tokio-tungstenite
  for WebSockets), or **Go** (quic-go). Both have mature HTTP/3 stacks.
* Lives on the gaming host as a system service. Users with both a Sunshine
  install and the proxy installed get a fully local pipeline; users who
  use GeForce Experience can run the proxy alongside.
* Optional but useful:
  * A mDNS broadcaster (`_moonlight._tcp.local`) so the PWA can discover
    proxies on the LAN.
  * A `/health` endpoint for the PWA to verify the proxy is reachable
    before kicking off the WebSocket session.

## Why not run the proxy in the same browser tab?

* The browser can't do raw UDP. Even with WebTransport, the remote end
  must speak HTTP/3 - GameStream / Sunshine do not.
* Chrome's Isolated Web Apps + Direct Sockets API can do raw UDP but
  require IWA install + an enterprise policy flag. Not suitable for a
  general-purpose PWA.

See `../PLAN.md` for the full architectural picture.
