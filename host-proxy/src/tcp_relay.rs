use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::frame::{encode_closed, encode_data, CLOSE_REASON_CONNECT_FAIL, CLOSE_REASON_NORMAL};

const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Spawn a TCP relay task for a single channel.
///
/// Connects to the target host:port, then spawns a read task to forward
/// TCP data back to the client, while the main loop forwards client data
/// to the TCP socket.
pub fn spawn(
    channel: u8,
    host: String,
    port: u16,
    frame_tx: mpsc::UnboundedSender<Vec<u8>>,
    is_stream: bool,
    data_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = relay(channel, &host, port, frame_tx, is_stream, data_rx, cancel).await {
            warn!("tcp relay ch={channel} error: {e}");
        }
    })
}

async fn relay(
    channel: u8,
    host: &str,
    port: u16,
    frame_tx: mpsc::UnboundedSender<Vec<u8>>,
    is_stream: bool,
    mut data_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    cancel: CancellationToken,
) -> Result<()> {
    let target = format!("{host}:{port}");
    debug!("tcp ch={channel} connecting to {target}");

    let stream = tokio::time::timeout(TCP_CONNECT_TIMEOUT, TcpStream::connect(&target))
        .await
        .map_err(|_| {
            let _ = frame_tx.send(encode_closed(channel, CLOSE_REASON_CONNECT_FAIL));
            anyhow::anyhow!("tcp connect timeout: {target}")
        })??;

    stream.set_nodelay(true)?;

    debug!("tcp ch={channel} connected to {target}");

    let (mut tcp_read, mut tcp_write) = stream.into_split();

    // Read task: TCP -> proxy -> client.
    // When the remote host closes the connection (EOF), we send a CLOSED
    // frame immediately so the client can signal EOF to the RTSP code via
    // recv() returning 0. We also cancel the write loop so this relay
    // shuts down cleanly without waiting for the client to send more data.
    let read_cancel = cancel.clone();
    let read_tx = frame_tx.clone();
    let write_cancel = cancel.clone();
    let read_handle = tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            tokio::select! {
                _ = read_cancel.cancelled() => break,
                result = tcp_read.read(&mut buf) => {
                    match result {
                        Ok(0) => {
                            debug!("tcp ch={channel} eof from host");
                            let _ = read_tx.send(encode_closed(channel, CLOSE_REASON_NORMAL));
                            write_cancel.cancel();
                            break;
                        }
                        Ok(n) => {
                            let frame = encode_data(channel, &buf[..n], is_stream);
                            let _ = read_tx.send(frame);
                        }
                        Err(e) => {
                            warn!("tcp read error ch={channel}: {e}");
                            let _ = read_tx.send(encode_closed(channel, CLOSE_REASON_NORMAL));
                            write_cancel.cancel();
                            break;
                        }
                    }
                }
            }
        }
    });

    // Write loop: client -> proxy -> TCP.
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            maybe_data = data_rx.recv() => {
                match maybe_data {
                    Some(data) => {
                        if let Err(e) = tcp_write.write_all(&data).await {
                            warn!("tcp write error ch={channel}: {e}");
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    read_handle.abort();

    Ok(())
}
