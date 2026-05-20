// WebTransport session handler.
//
// TODO: WebTransport (HTTP/3 over QUIC) support requires wtransport crate
// which needs Rust >= 1.88. The current toolchain is 1.85.
//
// For now, only the WebSocket path is active. This is acceptable per the
// README: "fall back to a single binary WebSocket otherwise."
//
// When Rust is upgraded, add wtransport to Cargo.toml and implement:
//   - QUIC endpoint setup in main.rs
//   - WT session accept, bidirectional stream + datagram reader
//   - Frame parsing from stream (using reserved field for DATA length)
//
// The frame protocol, channel manager, and relay tasks are unchanged between
// WS and WT — only the transport framing differs.

use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::Config;

#[allow(dead_code)]
pub async fn run(_config: Config, _cancel: CancellationToken) {
    warn!("WebTransport is not yet available — requires Rust >= 1.88");
}
