use anyhow::Result;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::frame::{encode_closed, encode_data, CLOSE_REASON_NORMAL};

/// Spawn a UDP relay task for a single channel.
///
/// Binds an ephemeral UDP socket, connects to the target, then spawns
/// concurrent send/recv loops to mirror traffic bidirectionally.
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
        if let Err(e) =
            relay(channel, &host, port, frame_tx, is_stream, data_rx, cancel).await
        {
            warn!("udp relay ch={channel} error: {e}");
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
    debug!("udp ch={channel} resolving {target}");

    let addr = tokio::net::lookup_host(&target)
        .await?
        .next()
        .ok_or_else(|| anyhow::anyhow!("no address for {target}"))?;

    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    // Deliberately NOT connect()-ing. On Windows, a connected UDP socket
    // surfaces ICMP "port unreachable" responses as WSAECONNRESET (10054),
    // which kills our recv loop. Even on an unconnected socket Windows can
    // still raise WSAECONNRESET on recv_from() after a prior send_to()
    // triggered an ICMP unreachable — unless SIO_UDP_CONNRESET is disabled,
    // or we just ignore that specific error (which we do below).

    let _ = socket2::SockRef::from(&socket).set_recv_buffer_size(256 * 1024);
    let _ = socket2::SockRef::from(&socket).set_send_buffer_size(256 * 1024);

    // Windows: disable SIO_UDP_CONNRESET so ICMP-unreachable replies don't
    // surface as WSAECONNRESET on the next recv_from(). This is the proper
    // fix; the error-skip in the recv loop below is a belt-and-braces.
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        const SIO_UDP_CONNRESET: u32 = 0x9800000C; // _WSAIOW(IOC_VENDOR, 12)
        let raw = socket.as_raw_socket() as usize;
        let mut new_behavior: u32 = 0;
        let mut bytes_returned: u32 = 0;
        // SAFETY: calling WSAIoctl on a valid SOCKET handle with a u32 in/out.
        unsafe {
            extern "system" {
                fn WSAIoctl(
                    s: usize,
                    dw_io_control_code: u32,
                    lpv_in_buffer: *mut std::ffi::c_void,
                    cb_in_buffer: u32,
                    lpv_out_buffer: *mut std::ffi::c_void,
                    cb_out_buffer: u32,
                    lpcb_bytes_returned: *mut u32,
                    lp_overlapped: *mut std::ffi::c_void,
                    lp_completion_routine: *mut std::ffi::c_void,
                ) -> i32;
            }
            let _ = WSAIoctl(
                raw,
                SIO_UDP_CONNRESET,
                &mut new_behavior as *mut _ as *mut _,
                std::mem::size_of::<u32>() as u32,
                std::ptr::null_mut(),
                0,
                &mut bytes_returned,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
    }

    debug!("udp ch={channel} bound to {} -> {addr}", socket.local_addr()?);

    let socket = std::sync::Arc::new(socket);

    // Recv task: reads from UDP socket, forwards to client.
    let recv_sock = socket.clone();
    let recv_cancel = cancel.clone();
    let recv_tx = frame_tx.clone();
    let recv = tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            tokio::select! {
                _ = recv_cancel.cancelled() => break,
                result = recv_sock.recv_from(&mut buf) => {
                    match result {
                        Ok((n, _src)) => {
                            // Sunshine should be the only sender on this
                            // ephemeral port; accept and forward.
                            let frame = encode_data(channel, &buf[..n], is_stream);
                            let _ = recv_tx.send(frame);
                        }
                        Err(e) => {
                            // Don't tear down the channel on transient
                            // errors. Windows can still throw WSAECONNRESET
                            // (10054) after an ICMP unreachable even with
                            // SIO_UDP_CONNRESET disabled, and on Linux
                            // similar ECONNREFUSED can surface. The cancel
                            // token is the only legitimate exit.
                            warn!("udp recv error ch={channel}: {e} (continuing)");
                            continue;
                        }
                    }
                }
            }
        }
    });

    // Write loop: forwards client data to the UDP socket (explicit dest).
    let exit_reason: &'static str;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => { exit_reason = "cancel"; break; }
            maybe_data = data_rx.recv() => {
                match maybe_data {
                    Some(data) => {
                        if let Err(e) = socket.send_to(&data, addr).await {
                            warn!("udp send error ch={channel}: {e}");
                            exit_reason = "send_err";
                            break;
                        }
                    }
                    None => { exit_reason = "data_tx_dropped"; break; }
                }
            }
        }
    }

    warn!("udp ch={channel} closing (reason={exit_reason})");
    recv.abort();
    let _ = frame_tx.send(encode_closed(channel, CLOSE_REASON_NORMAL));

    Ok(())
}
