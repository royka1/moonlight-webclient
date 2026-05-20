// mDNS broadcaster for LAN discovery.
//
// TODO: simple-mdns 0.7 API changed from the 0.6 Responder/Service pattern.
// Implement using either:
//   - simple-mdns async_discovery feature (requires "async-tokio")
//   - Raw UDP socket on port 5353 responding to PTR queries for _moonlight._tcp.local
//
// For now, mDNS is disabled. LAN discovery still works by manually entering the
// proxy's IP:port in the PWA.

use tokio_util::sync::CancellationToken;
use tracing::info;

pub fn start(port: u16, _cancel: CancellationToken) {
    info!("mDNS not yet implemented — proxy running on port {port} without LAN advertisement");
}
