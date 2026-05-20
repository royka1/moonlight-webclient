// Multiplexed transport to the host-side proxy.
//
// Protocol (binary, little-endian unless noted):
//
//   client -> proxy:
//     [u8 op][u8 channel][u16 reserved][payload]
//        op = 1  OPEN    payload = [u8 proto][u16 port][cstring host]
//        op = 2  CLOSE   payload = (none)
//        op = 3  DATA    payload = raw bytes destined for the host
//
//   proxy -> client:
//     [u8 op][u8 channel][u16 reserved][payload]
//        op = 3  DATA    payload = raw bytes received from the host
//        op = 4  CLOSED  payload = [u8 reason]
//
// Channel 0 is reserved for control / heartbeats. Channels 1..N are
// allocated by platform_web.c.
//
// Prefer WebTransport (HTTP/3 unreliable datagrams) where available; fall
// back to a single binary WebSocket.

const OP_OPEN = 1;
const OP_CLOSE = 2;
const OP_DATA = 3;
const OP_CLOSED = 4;

export type PacketHandler = (channel: number, data: Uint8Array) => void;

export class ProxyTransport {
  private ws?: WebSocket;
  private wt?: WebTransport;
  private wtWriter?: WritableStreamDefaultWriter<Uint8Array>;
  onPacket: PacketHandler = () => {};

  constructor(public url: string) {}

  async connect(): Promise<void> {
    if (this.url.startsWith('https://') && 'WebTransport' in self) {
      await this.connectWebTransport();
    } else {
      await this.connectWebSocket();
    }
  }

  private async connectWebTransport() {
    this.wt = new WebTransport(this.url);
    await this.wt.ready;
    // Open a bidirectional stream we'll keep alive for the session.
    const stream = await this.wt.createBidirectionalStream();
    this.wtWriter = stream.writable.getWriter();
    this.pumpReader(stream.readable.getReader());
    // Also pump unreliable datagrams - we'll prefer those for media when
    // the host proxy supports it.
    this.pumpDatagrams();
  }

  private async pumpReader(reader: ReadableStreamDefaultReader<Uint8Array>) {
    // TS 5.7+ tightened the Uint8Array generic; using ArrayBufferLike here
    // keeps us compatible with chunks returned by WebTransport (which may
    // be SAB-backed) and with subarray() results.
    let buffer: Uint8Array<ArrayBufferLike> = new Uint8Array(0);
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      buffer = concat(buffer, value!);
      while (buffer.length >= 4) {
        const len = lengthOfFrame(buffer);
        if (len < 0 || buffer.length < len) break;
        this.handleFrame(buffer.subarray(0, len));
        buffer = buffer.subarray(len);
      }
    }
  }

  private async pumpDatagrams() {
    if (!this.wt) return;
    const reader = this.wt.datagrams.readable.getReader();
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      if (value) this.handleFrame(value);
    }
  }

  private async connectWebSocket() {
    return new Promise<void>((resolve, reject) => {
      this.ws = new WebSocket(this.url);
      this.ws.binaryType = 'arraybuffer';
      this.ws.onopen = () => resolve();
      this.ws.onerror = (e) => reject(new Error(`ws error: ${e}`));
      this.ws.onmessage = (m) => {
        if (m.data instanceof ArrayBuffer) {
          this.handleFrame(new Uint8Array(m.data));
        }
      };
    });
  }

  private handleFrame(frame: Uint8Array) {
    if (frame.length < 4) return;
    const op = frame[0];
    const channel = frame[1];
    const payload = frame.subarray(4);
    switch (op) {
      case OP_DATA:
        this.onPacket(channel, payload);
        break;
      case OP_CLOSED: {
        // Remote host closed the connection; deliver an empty packet to
        // signal EOF. platform_web.c's mlw_inbound_packet treats len=0
        // as EOF for the channel, which makes recv() return 0.
        // payload[0] is the reason byte (NORMAL/BAD_PROTO/LIMIT/...)
        const reason = payload.length > 0 ? payload[0] : 255;
        console.info(`[transport] CLOSED ch=${channel} reason=${reason}`);
        this.onPacket(channel, new Uint8Array(0));
        break;
      }
    }
  }

  openChannel(channel: number, host: string, port: number, proto: number) {
    const hostBytes = new TextEncoder().encode(host);
    const buf = new Uint8Array(4 + 1 + 2 + hostBytes.length + 1);
    buf[0] = OP_OPEN; buf[1] = channel;
    buf[4] = proto;
    buf[5] = port & 0xff; buf[6] = (port >> 8) & 0xff;
    buf.set(hostBytes, 7);
    buf[7 + hostBytes.length] = 0;
    this.write(buf);
  }

  sendChannel(channel: number, data: Uint8Array): number {
    const buf = new Uint8Array(4 + data.length);
    buf[0] = OP_DATA; buf[1] = channel;
    buf[2] = data.length & 0xff;
    buf[3] = (data.length >> 8) & 0xff;
    buf.set(data, 4);
    this.write(buf);
    return data.length;
  }

  closeChannel(channel: number) {
    const buf = new Uint8Array(4);
    buf[0] = OP_CLOSE; buf[1] = channel;
    this.write(buf);
  }

  private write(buf: Uint8Array) {
    if (this.wtWriter) {
      this.wtWriter.write(buf).catch((e) => console.warn('[transport] wt write', e));
    } else if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(buf);
    }
  }

  close() {
    this.wtWriter?.close().catch(() => {});
    this.wt?.close({ closeCode: 0, reason: 'client closing' });
    this.ws?.close();
  }
}

function concat(
  a: Uint8Array<ArrayBufferLike>,
  b: Uint8Array<ArrayBufferLike>,
): Uint8Array<ArrayBufferLike> {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

// Frames over WebTransport streams use a single u16 length prefix prepended
// by the proxy when streaming TCP-like data. UDP-like data arrives as
// datagrams already framed at the wire boundary.
function lengthOfFrame(buf: Uint8Array): number {
  if (buf.length < 4) return -1;
  // Our control frames are self-delimiting (header op = 1 byte, channel = 1,
  // reserved = 2, then payload). For DATA frames over a TCP-like stream we
  // expect the proxy to prefix u16 length in the reserved field. The proxy
  // implementation in host-proxy/ honours this convention.
  return 4 + (buf[2] | (buf[3] << 8));
}
