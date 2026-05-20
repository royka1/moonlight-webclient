use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::frame::{
    encode_closed, parse_open_payload, FrameHeader, CLOSE_REASON_BAD_PROTO,
    CLOSE_REASON_LIMIT, OP_CLOSE, OP_DATA, OP_OPEN, PROTO_TCP, PROTO_UDP,
};
use crate::tcp_relay;
use crate::udp_relay;

pub struct ChannelManager {
    channels: HashMap<u8, (mpsc::UnboundedSender<Vec<u8>>, JoinHandle<()>)>,
    frame_tx: mpsc::UnboundedSender<Vec<u8>>,
    is_stream: bool,
    max_channels: u8,
    cancel: CancellationToken,
}

impl ChannelManager {
    pub fn new(
        frame_tx: mpsc::UnboundedSender<Vec<u8>>,
        is_stream: bool,
        max_channels: u8,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            channels: HashMap::new(),
            frame_tx,
            is_stream,
            max_channels,
            cancel,
        }
    }

    /// Dispatch an incoming frame from the client.
    pub fn dispatch(&mut self, header: FrameHeader, payload: &[u8]) {
        match header.op {
            OP_OPEN => self.handle_open(header.channel, payload),
            OP_CLOSE => self.handle_close(header.channel),
            OP_DATA => self.handle_data(header.channel, payload),
            _ => {
                debug!("unknown op {} on ch={}", header.op, header.channel);
            }
        }
    }

    fn handle_open(&mut self, channel: u8, payload: &[u8]) {
        // Validate channel ID (channel 0 is reserved for control).
        if channel == 0 {
            warn!("client tried to open reserved channel 0");
            let _ = self.frame_tx.send(encode_closed(0, CLOSE_REASON_BAD_PROTO));
            return;
        }

        // Check channel limit.
        if self.channels.len() >= self.max_channels as usize {
            warn!("channel limit {} reached", self.max_channels);
            let _ = self
                .frame_tx
                .send(encode_closed(channel, CLOSE_REASON_LIMIT));
            return;
        }

        // Check for duplicate channel.
        if self.channels.contains_key(&channel) {
            warn!("duplicate open for ch={channel}");
            let _ = self.frame_tx.send(encode_closed(channel, CLOSE_REASON_BAD_PROTO));
            return;
        }

        // Parse the OPEN payload.
        let open = match parse_open_payload(payload) {
            Some(o) => o,
            None => {
                warn!("malformed OPEN payload on ch={channel}");
                let _ = self
                    .frame_tx
                    .send(encode_closed(channel, CLOSE_REASON_BAD_PROTO));
                return;
            }
        };

        debug!(
            "open ch={channel} proto={} host={}:{}",
            open.proto, open.host, open.port
        );

        // Spawn the appropriate relay.
        let (data_tx, data_rx) = mpsc::unbounded_channel();
        let handle = match open.proto {
            PROTO_UDP => udp_relay::spawn(
                channel,
                open.host,
                open.port,
                self.frame_tx.clone(),
                self.is_stream,
                data_rx,
                self.cancel.child_token(),
            ),
            PROTO_TCP => tcp_relay::spawn(
                channel,
                open.host,
                open.port,
                self.frame_tx.clone(),
                self.is_stream,
                data_rx,
                self.cancel.child_token(),
            ),
            _ => {
                warn!("unknown proto {} on ch={channel}", open.proto);
                let _ = self
                    .frame_tx
                    .send(encode_closed(channel, CLOSE_REASON_BAD_PROTO));
                return;
            }
        };

        self.channels.insert(channel, (data_tx, handle));
    }

    fn handle_close(&mut self, channel: u8) {
        warn!("close ch={channel} (initiated by client)");
        if let Some((_data_tx, handle)) = self.channels.remove(&channel) {
            // Dropping data_tx signals the relay to stop.
            handle.abort();
        }
    }

    fn handle_data(&self, channel: u8, payload: &[u8]) {
        match self.channels.get(&channel) {
            Some((data_tx, _)) => {
                // Forward to the relay task.
                if data_tx.send(payload.to_vec()).is_err() {
                    debug!("data for dead channel ch={channel}, dropping");
                }
            }
            None => {
                // Channel may have been closed already — silent drop.
                debug!("data for unknown ch={channel}, dropping");
            }
        }
    }

    /// Clean up all active channels.
    pub fn close_all(&mut self) {
        for (ch, (_data_tx, handle)) in self.channels.drain() {
            debug!("force-closing ch={ch}");
            handle.abort();
        }
    }
}

impl Drop for ChannelManager {
    fn drop(&mut self) {
        self.close_all();
    }
}
