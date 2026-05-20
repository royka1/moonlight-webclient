import type { Capabilities } from '../capabilities';
import type { Host } from '../client/host-store';
import type { LaunchResult } from '../client/nvhttp';
import { MoonlightClient, type StreamConfig, type LaunchInfo } from '../client/moonlight-client';
import { VideoSurface } from '../video/webcodecs-decoder';
import { AudioRenderer } from '../audio/audio-renderer';
import { KeyboardInput } from '../input/keyboard';
import { PointerInput } from '../input/pointer';
import { GamepadInput } from '../input/gamepad';

const DEFAULT_CONFIG: StreamConfig = {
  width: 1920,
  height: 1080,
  fps: 60,
  bitrateKbps: 20000,
  codec: 'h264',
  audioConfiguration: 'stereo',
  packetSize: 1392,
};

export class StreamView {
  private root: HTMLDivElement;
  private overlay: HTMLDivElement;

  private client?: MoonlightClient;
  private video?: VideoSurface;
  private audio?: AudioRenderer;
  private keyboard?: KeyboardInput;
  private pointer?: PointerInput;
  private gamepad?: GamepadInput;

  private statsInterval?: number;
  private overlayHideTimer?: number;

  private showStats: boolean;

  constructor(
    private parent: HTMLElement,
    private host: Host,
    private caps: Capabilities,
    // Launch is now fired AFTER the wasm is loaded and the proxy ws is
    // open, so callers pass a thunk instead of a pre-fetched result.
    private launchFn: () => Promise<LaunchResult>,
    opts: { showStats?: boolean } = {},
  ) {
    this.showStats = opts.showStats ?? true;
    this.root = document.createElement('div');
    this.root.className = 'stream-view';
    this.root.tabIndex = 0;

    this.overlay = document.createElement('div');
    this.overlay.className = 'overlay';
    this.overlay.textContent = 'Connecting...';
    this.root.appendChild(this.overlay);

    this.parent.appendChild(this.root);
  }

  async start(overrides: Partial<StreamConfig> = {}) {
    const config: StreamConfig = { ...DEFAULT_CONFIG, ...overrides };
    console.info('[stream] starting: %dx%d @ %d fps, %d kbps, codec=%s',
      config.width, config.height, config.fps, config.bitrateKbps, config.codec);

    await this.root.requestFullscreen().catch(() => {
      console.warn('[stream] fullscreen denied; continuing windowed');
    });

    // Build the surface (<video> + MSTG) on main, decoder lives in the worker.
    this.video = new VideoSurface(this.root, config);
    const writable = this.video.init();
    if (!writable) {
      throw new Error('MediaStreamTrackGenerator unavailable; canvas fallback not implemented');
    }

    this.audio = new AudioRenderer();
    await this.audio.init();

    this.client = new MoonlightClient({
      host: this.host,
      config,
      onAudioFrame: (samples) => this.audio!.submit(samples),
      onStatus: (msg) => {
        this.overlay.textContent = msg;
      },
      onTerminated: (err) => this.stop(err),
      onVideoStats: (s) => this.video?.updateStats(s),
    });

    // Hand the writable side of the MSTG to the worker so the in-worker
    // VideoDecoder writes decoded frames directly into the track. From this
    // point on no chunk data crosses the worker↔main boundary.
    this.client.attachVideoDecoder(writable, config.codec, config.width, config.height, config.fps);

    // Hook input BEFORE starting so the keyboard lock prompt happens during
    // the user-gesture window (the fullscreen request above is the gesture).
    this.keyboard = new KeyboardInput(this.root, this.client, this.caps);
    this.pointer = new PointerInput(this.video.element, this.client);
    this.gamepad = new GamepadInput(this.client);

    await this.keyboard.attach();
    this.pointer.attach();
    this.gamepad.start();

    // Critical ordering: load wasm + open proxy ws FIRST, then /launch
    // on the host, then immediately mlw_start. Sunshine's launch session
    // expires 10s after /launch if no RTSP TCP connect arrives.
    this.overlay.textContent = 'Loading streaming engine…';
    await this.client.prepare();

    this.overlay.textContent = 'Launching app on host…';
    const launch: LaunchInfo = await this.launchFn();

    this.overlay.textContent = 'Connecting RTSP…';
    await this.client.start(launch);

    if (this.showStats) {
      this.statsInterval = window.setInterval(() => this.updateStats(), 500);
      // Show the stats overlay only briefly on mouse-move; ChromeOS keeps
      // the <video> on a hardware-overlay plane more aggressively when no
      // opaque sibling compositor layer is fighting for the same region.
      this.root.addEventListener('mousemove', () => this.bumpOverlay());
      this.bumpOverlay();
    } else {
      // User turned the overlay off; hide it permanently for this session.
      this.overlay.classList.add('hidden');
      this.overlay.style.display = 'none';
    }
  }

  private bumpOverlay() {
    if (!this.showStats) return;
    this.overlay.classList.remove('hidden');
    if (this.overlayHideTimer) clearTimeout(this.overlayHideTimer);
    this.overlayHideTimer = window.setTimeout(
      () => this.overlay.classList.add('hidden'),
      3000,
    );
  }

  private updateStats() {
    if (!this.video) return;
    const v = this.video.stats();
    const a = this.audio?.stats();
    this.overlay.textContent =
      `${v.width}x${v.height} @ ${v.fps.toFixed(0)} fps · ` +
      `decode ${v.decodeLatencyMs.toFixed(1)} ms · ` +
      `render ${v.renderLatencyMs.toFixed(1)} ms · ` +
      `present ${v.presentLatencyMs.toFixed(1)} ms · ` +
      `dropped ${v.dropped} · ` +
      `audio queue ${a?.queuedMs.toFixed(0) ?? '?'} ms`;
  }

  async stop(reason?: unknown) {
    clearInterval(this.statsInterval);
    if (this.overlayHideTimer) clearTimeout(this.overlayHideTimer);
    this.gamepad?.stop();
    this.keyboard?.detach();
    this.pointer?.detach();
    await this.client?.disconnect();
    this.audio?.close();
    this.video?.close();
    if (document.fullscreenElement) await document.exitFullscreen().catch(() => {});
    this.root.remove();
    if (reason) console.warn('[stream] terminated:', reason);
  }
}
