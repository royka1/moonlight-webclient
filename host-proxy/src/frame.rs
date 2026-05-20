// Binary wire protocol for the multiplexed transport.
// Must match src/transport/proxy-transport.ts byte-for-byte.
//
// Frame header (4 bytes, little-endian):
//   [u8 op][u8 channel][u16 reserved][payload]
//
// Client -> Proxy:
//   op=1 OPEN:   payload = [u8 proto][u16 port LE][cstring host]
//   op=2 CLOSE:  payload = (none)
//   op=3 DATA:   payload = raw bytes
//
// Proxy -> Client:
//   op=3 DATA:   payload = raw bytes
//   op=4 CLOSED: payload = [u8 reason]

pub const OP_OPEN: u8 = 1;
pub const OP_CLOSE: u8 = 2;
pub const OP_DATA: u8 = 3;
pub const OP_CLOSED: u8 = 4;

pub const PROTO_UDP: u8 = 1;
pub const PROTO_TCP: u8 = 2;

#[allow(dead_code)]
pub const CHANNEL_CONTROL: u8 = 0;

pub const CLOSE_REASON_NORMAL: u8 = 0;
pub const CLOSE_REASON_BAD_PROTO: u8 = 1;
pub const CLOSE_REASON_LIMIT: u8 = 2;
pub const CLOSE_REASON_CONNECT_FAIL: u8 = 3;
#[allow(dead_code)]
pub const CLOSE_REASON_BIND_FAIL: u8 = 4;
#[allow(dead_code)]
pub const CLOSE_REASON_IO_ERROR: u8 = 5;

/// Parsed frame header from the first 4 bytes.
#[derive(Debug, Clone, Copy)]
pub struct FrameHeader {
    pub op: u8,
    pub channel: u8,
    #[allow(dead_code)]
    pub reserved: u16,
}

/// Payload for an OPEN frame.
#[derive(Debug, Clone)]
pub struct OpenPayload {
    pub proto: u8,
    pub port: u16,
    pub host: String,
}

/// Encode an OPEN frame.
/// Byte layout: [OP_OPEN][ch][0][0][proto][port_lo][port_hi][host_bytes...][0]
#[allow(dead_code)]
pub fn encode_open(channel: u8, proto: u8, port: u16, host: &str) -> Vec<u8> {
    let host_bytes = host.as_bytes();
    let mut buf = Vec::with_capacity(4 + 1 + 2 + host_bytes.len() + 1);
    buf.push(OP_OPEN);
    buf.push(channel);
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.push(proto);
    buf.extend_from_slice(&port.to_le_bytes());
    buf.extend_from_slice(host_bytes);
    buf.push(0);
    buf
}

/// Encode a CLOSE frame (exactly 4 bytes).
#[allow(dead_code)]
pub fn encode_close(channel: u8) -> Vec<u8> {
    vec![OP_CLOSE, channel, 0, 0]
}

/// Encode a DATA frame.
/// If set_len is true, the reserved field carries the payload length (for WT stream mode).
/// For WebSocket and datagram delivery, set_len should be false since framing is external.
pub fn encode_data(channel: u8, payload: &[u8], set_len: bool) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.push(OP_DATA);
    buf.push(channel);
    if set_len {
        let len = (payload.len() as u16).to_le_bytes();
        buf.extend_from_slice(&len);
    } else {
        buf.extend_from_slice(&0u16.to_le_bytes());
    }
    buf.extend_from_slice(payload);
    buf
}

/// Encode a CLOSED frame sent from proxy to client.
/// Byte layout: [OP_CLOSED][ch][0][0][reason]
pub fn encode_closed(channel: u8, reason: u8) -> Vec<u8> {
    vec![OP_CLOSED, channel, 0, 0, reason]
}

/// Parse a frame header from a buffer. Returns None if too short.
pub fn parse_frame_header(buf: &[u8]) -> Option<FrameHeader> {
    if buf.len() < 4 {
        return None;
    }
    Some(FrameHeader {
        op: buf[0],
        channel: buf[1],
        reserved: u16::from_le_bytes([buf[2], buf[3]]),
    })
}

/// Parse an OPEN frame payload. Returns None if malformed.
/// Payload format: [u8 proto][u16 port LE][cstring host]
pub fn parse_open_payload(payload: &[u8]) -> Option<OpenPayload> {
    if payload.len() < 4 {
        return None;
    }
    let proto = payload[0];
    let port = u16::from_le_bytes([payload[1], payload[2]]);
    // Find NUL terminator for the host cstring
    let nul_pos = payload[3..].iter().position(|&b| b == 0)?;
    let host_bytes = &payload[3..3 + nul_pos];
    let host = String::from_utf8(host_bytes.to_vec()).ok()?;
    Some(OpenPayload { proto, port, host })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_roundtrip() {
        let buf = encode_open(5, PROTO_UDP, 47998, "192.168.1.100");
        let hdr = parse_frame_header(&buf).unwrap();
        assert_eq!(hdr.op, OP_OPEN);
        assert_eq!(hdr.channel, 5);
        let payload = parse_open_payload(&buf[4..]).unwrap();
        assert_eq!(payload.proto, PROTO_UDP);
        assert_eq!(payload.port, 47998);
        assert_eq!(payload.host, "192.168.1.100");
    }

    #[test]
    fn test_close_frame() {
        let buf = encode_close(3);
        let hdr = parse_frame_header(&buf).unwrap();
        assert_eq!(hdr.op, OP_CLOSE);
        assert_eq!(hdr.channel, 3);
        assert_eq!(buf.len(), 4);
    }

    #[test]
    fn test_data_with_len() {
        let payload = b"hello";
        let buf = encode_data(1, payload, true);
        let hdr = parse_frame_header(&buf).unwrap();
        assert_eq!(hdr.op, OP_DATA);
        assert_eq!(hdr.channel, 1);
        assert_eq!(hdr.reserved, 5);
        assert_eq!(&buf[4..], payload);
    }

    #[test]
    fn test_data_no_len() {
        let payload = b"world";
        let buf = encode_data(2, payload, false);
        let hdr = parse_frame_header(&buf).unwrap();
        assert_eq!(hdr.reserved, 0);
        assert_eq!(&buf[4..], payload);
    }

    #[test]
    fn test_closed_frame() {
        let buf = encode_closed(7, CLOSE_REASON_NORMAL);
        assert_eq!(buf.len(), 5);
        assert_eq!(buf[0], OP_CLOSED);
        assert_eq!(buf[1], 7);
        assert_eq!(buf[4], CLOSE_REASON_NORMAL);
    }

    #[test]
    fn test_length_of_frame() {
        // Simulates TypeScript lengthOfFrame: 4 + (buf[2] | (buf[3] << 8))
        let data = encode_data(1, &vec![0u8; 100], true);
        let len = 4 + u16::from_le_bytes([data[2], data[3]]) as usize;
        assert_eq!(len, 104); // 4 header + 100 payload
    }

    #[test]
    fn test_open_payload_missing_nul() {
        let payload = [1u8, 0x22, 0xBB, 0x41]; // proto=1, port=0xBB22, no NUL
        assert!(parse_open_payload(&payload).is_none());
    }

    #[test]
    fn test_short_header() {
        assert!(parse_frame_header(&[1, 2]).is_none());
    }
}
