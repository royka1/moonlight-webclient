use axum::extract::ws::{Message, WebSocket};
use futures_util::SinkExt;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::channel::ChannelManager;
use crate::frame::parse_frame_header;
use crate::Config;

/// Run a WebSocket session. Called after axum's WebSocket upgrade.
pub async fn run(ws: WebSocket, config: Config, cancel: CancellationToken) {
    let (mut ws_write, mut ws_read) = ws.split();

    // Channel for relay tasks to send frames back to client.
    let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Writer task: reads encoded frames from mpsc and writes to WebSocket.
    let write_handle = tokio::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            if ws_write.send(Message::Binary(frame.into())).await.is_err() {
                break;
            }
        }
    });

    let mut channels = ChannelManager::new(
        frame_tx.clone(),
        false, // is_stream: false — WS messages are self-delimiting
        config.max_channels,
        cancel.clone(),
    );

    info!("ws session started");

    // Read loop: each binary WS message is one complete frame.
    while let Some(msg) = ws_read.next().await {
        match msg {
            Ok(Message::Binary(data)) => {
                if let Some(header) = parse_frame_header(&data) {
                    channels.dispatch(header, &data[4..]);
                } else {
                    warn!("ws frame too short: {} bytes", data.len());
                }
            }
            Ok(Message::Ping(_)) => {
                // axum handles pong automatically
            }
            Ok(Message::Close(_)) => {
                debug!("ws client close frame");
                break;
            }
            Ok(Message::Pong(_)) | Ok(Message::Text(_)) => {}
            Err(e) => {
                warn!("ws read error: {e}");
                break;
            }
        }
    }

    info!("ws session closing");
    channels.close_all();
    drop(frame_tx); // Signal writer to stop.
    write_handle.abort();
}
