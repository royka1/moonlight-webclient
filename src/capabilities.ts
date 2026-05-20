// Feature detection - run once at boot so the UI can warn early if the
// runtime can't actually stream.

export interface Capabilities {
  crossOriginIsolated: boolean;
  sharedArrayBuffer: boolean;
  webCodecs: boolean;
  h264Hardware: boolean;
  hevcHardware: boolean;
  av1Hardware: boolean;
  webTransport: boolean;
  webSockets: boolean;
  keyboardLock: boolean;
  pointerLock: boolean;
  gamepad: boolean;
  audioWorklet: boolean;
  fullscreen: boolean;
  installed: boolean;
}

async function checkCodecSupport(config: VideoDecoderConfig): Promise<boolean> {
  if (typeof VideoDecoder === 'undefined') return false;
  try {
    const r = await VideoDecoder.isConfigSupported(config);
    return r.supported === true;
  } catch {
    return false;
  }
}

export async function detectCapabilities(): Promise<Capabilities> {
  const installed = window.matchMedia('(display-mode: standalone)').matches;

  // Probing codec configs forces the browser to consult the HW decoder list.
  // We pick conservative configs that any modern GPU should accept.
  const [h264, hevc, av1] = await Promise.all([
    checkCodecSupport({
      codec: 'avc1.640028', // High profile, level 4.0
      hardwareAcceleration: 'prefer-hardware',
      codedWidth: 1920,
      codedHeight: 1080,
    }),
    checkCodecSupport({
      codec: 'hev1.1.6.L120.90', // Main profile, level 4.0
      hardwareAcceleration: 'prefer-hardware',
      codedWidth: 1920,
      codedHeight: 1080,
    }),
    checkCodecSupport({
      codec: 'av01.0.08M.08',
      hardwareAcceleration: 'prefer-hardware',
      codedWidth: 1920,
      codedHeight: 1080,
    }),
  ]);

  return {
    crossOriginIsolated: self.crossOriginIsolated === true,
    sharedArrayBuffer: typeof SharedArrayBuffer !== 'undefined',
    webCodecs: typeof VideoDecoder !== 'undefined',
    h264Hardware: h264,
    hevcHardware: hevc,
    av1Hardware: av1,
    webTransport: typeof (window as any).WebTransport !== 'undefined',
    webSockets: typeof WebSocket !== 'undefined',
    keyboardLock: !!(navigator as any).keyboard?.lock,
    pointerLock: 'requestPointerLock' in HTMLElement.prototype,
    gamepad: typeof navigator.getGamepads === 'function',
    audioWorklet: typeof AudioWorkletNode !== 'undefined',
    fullscreen: 'requestFullscreen' in HTMLElement.prototype,
    installed,
  };
}
