/// <reference types="vite/client" />

// Asset imports via the ?url suffix.
declare module '*?url' {
  const src: string;
  export default src;
}

declare module '*.js?url' {
  const src: string;
  export default src;
}

// The emscripten-generated module sits at /wasm/moonlight.js (copied into
// `public/wasm/` by wasm/build.sh). We import it dynamically at runtime;
// declare the module so TypeScript stops complaining.
declare module '/wasm/moonlight.js' {
  type MoonlightFactory = (opts?: any) => Promise<any>;
  const factory: MoonlightFactory;
  export default factory;
  export const createMoonlightModule: MoonlightFactory;
}

// ---- WebTransport (Chromium 97+, Firefox 130+ behind a flag, Safari TP). ----
// As of TS 5.6 the lib.dom.d.ts definitions are still partial; declare just
// enough surface for our use.
interface WebTransportCloseInfo {
  closeCode?: number;
  reason?: string;
}

interface WebTransportBidirectionalStream {
  readable: ReadableStream<Uint8Array>;
  writable: WritableStream<Uint8Array>;
}

interface WebTransportDatagramDuplexStream {
  readable: ReadableStream<Uint8Array>;
  writable: WritableStream<Uint8Array>;
}

interface WebTransport {
  readonly ready: Promise<undefined>;
  readonly closed: Promise<WebTransportCloseInfo>;
  readonly datagrams: WebTransportDatagramDuplexStream;
  createBidirectionalStream(): Promise<WebTransportBidirectionalStream>;
  close(info?: WebTransportCloseInfo): void;
}

declare const WebTransport: {
  prototype: WebTransport;
  new (url: string, options?: unknown): WebTransport;
};

// ---- Keyboard Lock (Chromium, Firefox 130+) ----
interface KeyboardLockMethods {
  lock(keyCodes?: string[]): Promise<void>;
  unlock(): void;
}

interface Navigator {
  readonly keyboard?: KeyboardLockMethods;
}
