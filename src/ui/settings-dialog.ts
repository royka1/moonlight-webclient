import {
  AUDIO_OPTIONS,
  CODEC_OPTIONS,
  FPS_OPTIONS,
  RESOLUTION_OPTIONS,
  defaultBitrateMbps,
  isCustomResolution,
  loadSettings,
  saveSettings,
  type StreamSettings,
} from '../client/stream-settings';
import type { VideoCodec, AudioConfiguration } from '../client/moonlight-client';

const CUSTOM_RES = 'custom';

/**
 * Modal for editing the persistent streaming settings. Resolves when the
 * user closes the modal (the settings are saved on every change).
 */
export function openSettingsDialog(): Promise<void> {
  return new Promise<void>((resolve) => {
    const settings = loadSettings();

    const scrim = document.createElement('div');
    scrim.className = 'modal-scrim';
    const modal = document.createElement('div');
    modal.className = 'modal';
    modal.style.minWidth = '420px';

    const h2 = document.createElement('h2');
    h2.textContent = 'Streaming settings';
    modal.appendChild(h2);

    const grid = document.createElement('div');
    grid.style.display = 'grid';
    grid.style.gridTemplateColumns = 'auto 1fr';
    grid.style.gap = '10px 14px';
    grid.style.alignItems = 'center';
    modal.appendChild(grid);

    // Resolution: preset dropdown + "Custom" option that reveals two inputs.
    const startCustom = isCustomResolution(settings.width, settings.height);
    const resSel = makeSelect(
      [
        ...RESOLUTION_OPTIONS.map((r) => ({
          value: `${r.width}x${r.height}`,
          label: `${r.label} (${r.width}×${r.height})`,
        })),
        { value: CUSTOM_RES, label: 'Custom…' },
      ],
      startCustom ? CUSTOM_RES : `${settings.width}x${settings.height}`,
    );
    addRow(grid, 'Resolution', resSel);

    // Custom width × height inputs (revealed when "Custom…" is selected).
    const customWidth = document.createElement('input');
    customWidth.type = 'number';
    customWidth.min = '320';
    customWidth.max = '7680';
    customWidth.step = '2';
    customWidth.value = String(settings.width);
    customWidth.style.width = '80px';
    const customHeight = document.createElement('input');
    customHeight.type = 'number';
    customHeight.min = '240';
    customHeight.max = '4320';
    customHeight.step = '2';
    customHeight.value = String(settings.height);
    customHeight.style.width = '80px';
    const customWrap = document.createElement('div');
    customWrap.style.display = startCustom ? 'flex' : 'none';
    customWrap.style.alignItems = 'center';
    customWrap.style.gap = '6px';
    customWrap.appendChild(customWidth);
    const x = document.createElement('span');
    x.textContent = '×';
    x.style.color = 'var(--fg-muted)';
    customWrap.appendChild(x);
    customWrap.appendChild(customHeight);
    const customLabel = document.createElement('label');
    customLabel.textContent = 'Custom size';
    customLabel.style.fontSize = '13px';
    customLabel.style.color = 'var(--fg-muted)';
    customLabel.style.display = startCustom ? 'block' : 'none';
    grid.appendChild(customLabel);
    grid.appendChild(customWrap);

    // FPS
    const fpsSel = makeSelect(
      FPS_OPTIONS.map((v) => ({ value: String(v), label: `${v} fps` })),
      String(settings.fps),
    );
    addRow(grid, 'Frame rate', fpsSel);

    // Codec
    const codecSel = makeSelect(
      CODEC_OPTIONS.map((c) => ({ value: c.value, label: c.label })),
      settings.codec,
    );
    addRow(grid, 'Codec', codecSel);

    // Audio
    const audioSel = makeSelect(
      AUDIO_OPTIONS.map((a) => ({ value: a.value, label: a.label })),
      settings.audio,
    );
    addRow(grid, 'Audio', audioSel);

    // Bitrate
    const bitrateInput = document.createElement('input');
    bitrateInput.type = 'number';
    bitrateInput.min = '1';
    bitrateInput.max = '200';
    bitrateInput.step = '1';
    bitrateInput.value = String(settings.bitrateMbps);
    const bitrateWrap = document.createElement('div');
    bitrateWrap.style.display = 'flex';
    bitrateWrap.style.alignItems = 'center';
    bitrateWrap.style.gap = '8px';
    bitrateInput.style.width = '90px';
    bitrateWrap.appendChild(bitrateInput);
    const mbpsLabel = document.createElement('span');
    mbpsLabel.textContent = 'Mbps';
    mbpsLabel.style.color = 'var(--fg-muted)';
    mbpsLabel.style.fontSize = '13px';
    bitrateWrap.appendChild(mbpsLabel);
    const resetBtn = document.createElement('button');
    resetBtn.textContent = 'Auto';
    resetBtn.title = 'Recommended bitrate for the chosen resolution + fps';
    resetBtn.style.fontSize = '12px';
    resetBtn.style.padding = '4px 10px';
    bitrateWrap.appendChild(resetBtn);
    addRow(grid, 'Bitrate', bitrateWrap);

    // Show statistics overlay (FPS / latency / dropped frames).
    const statsCheckbox = document.createElement('input');
    statsCheckbox.type = 'checkbox';
    statsCheckbox.checked = settings.showStats;
    const statsWrap = document.createElement('label');
    statsWrap.style.display = 'flex';
    statsWrap.style.alignItems = 'center';
    statsWrap.style.gap = '8px';
    statsWrap.style.cursor = 'pointer';
    statsWrap.appendChild(statsCheckbox);
    const statsCaption = document.createElement('span');
    statsCaption.textContent = 'Show during streaming';
    statsCaption.style.fontSize = '13px';
    statsWrap.appendChild(statsCaption);
    addRow(grid, 'Statistics', statsWrap);

    const hint = document.createElement('p');
    hint.style.fontSize = '12px';
    hint.style.color = 'var(--fg-muted)';
    hint.style.margin = '10px 0 0';
    hint.textContent =
      'AV1/HEVC need hardware support on both ends. If unsure, leave H.264.';
    modal.appendChild(hint);

    const actions = document.createElement('div');
    actions.className = 'actions';
    const closeBtn = document.createElement('button');
    closeBtn.textContent = 'Done';
    closeBtn.classList.add('primary');
    actions.appendChild(closeBtn);
    modal.appendChild(actions);

    scrim.appendChild(modal);
    document.body.appendChild(scrim);

    function current(): StreamSettings {
      let w: number;
      let h: number;
      if (resSel.value === CUSTOM_RES) {
        w = Math.max(320, Math.min(7680, parseInt(customWidth.value, 10) || 1920));
        h = Math.max(240, Math.min(4320, parseInt(customHeight.value, 10) || 1080));
        // Round to even pixels — h264 SPS requires even dimensions and odd
        // sizes get rejected by Sunshine's encoder.
        if (w & 1) w++;
        if (h & 1) h++;
      } else {
        [w, h] = resSel.value.split('x').map((n) => parseInt(n, 10));
      }
      return {
        width: w,
        height: h,
        fps: parseInt(fpsSel.value, 10),
        codec: codecSel.value as VideoCodec,
        audio: audioSel.value as AudioConfiguration,
        bitrateMbps: Math.max(1, Math.min(500, parseInt(bitrateInput.value, 10) || 1)),
        showStats: statsCheckbox.checked,
      };
    }

    function persist() {
      saveSettings(current());
    }

    resSel.onchange = () => {
      const isCustom = resSel.value === CUSTOM_RES;
      customLabel.style.display = isCustom ? 'block' : 'none';
      customWrap.style.display = isCustom ? 'flex' : 'none';
      persist();
    };
    fpsSel.onchange = codecSel.onchange = audioSel.onchange = persist;
    customWidth.oninput = customHeight.oninput = persist;
    statsCheckbox.onchange = persist;
    bitrateInput.oninput = persist;

    resetBtn.onclick = () => {
      const c = current();
      bitrateInput.value = String(defaultBitrateMbps(c.width, c.height, c.fps));
      persist();
    };

    function finish() {
      persist();
      scrim.remove();
      resolve();
    }
    closeBtn.onclick = finish;
    scrim.addEventListener('click', (e) => {
      if (e.target === scrim) finish();
    });
  });
}

function makeSelect(
  options: { value: string; label: string }[],
  selected: string,
): HTMLSelectElement {
  const sel = document.createElement('select');
  sel.style.minWidth = '180px';
  for (const opt of options) {
    const o = document.createElement('option');
    o.value = opt.value;
    o.textContent = opt.label;
    if (opt.value === selected) o.selected = true;
    sel.appendChild(o);
  }
  return sel;
}

function addRow(grid: HTMLElement, label: string, control: HTMLElement) {
  const l = document.createElement('label');
  l.textContent = label;
  l.style.fontSize = '13px';
  l.style.color = 'var(--fg-muted)';
  grid.appendChild(l);
  grid.appendChild(control);
}
