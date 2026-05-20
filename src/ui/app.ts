import type { Capabilities } from '../capabilities';
import { StreamView } from './stream-view';
import { HostList } from './host-list';
import { openPairDialog } from './pair-dialog';
import { openAppPicker } from './app-picker';
import { openSettingsDialog } from './settings-dialog';
import { launchApp } from '../client/nvhttp';
import { type Host, loadHosts, saveHosts } from '../client/host-store';
import { getDeviceName, setDeviceName } from '../client/settings';
import { loadSettings, toStreamConfig } from '../client/stream-settings';

export function mountApp(root: HTMLElement, caps: Capabilities) {
  root.innerHTML = '';
  const shell = document.createElement('div');
  shell.className = 'shell';

  const header = document.createElement('header');
  const title = document.createElement('h1');
  title.textContent = 'Moonlight';
  header.appendChild(title);

  // "Device name" - how this PWA identifies itself to the gaming host.
  // Shows up under Sunshine's Paired Clients list.
  const deviceLabel = document.createElement('label');
  deviceLabel.style.marginLeft = '24px';
  deviceLabel.style.color = 'var(--fg-muted)';
  deviceLabel.style.fontSize = '13px';
  deviceLabel.textContent = 'Device name: ';

  const deviceInput = document.createElement('input');
  deviceInput.value = getDeviceName();
  deviceInput.style.marginLeft = '8px';
  deviceInput.style.width = '180px';
  deviceInput.oninput = () => setDeviceName(deviceInput.value);
  deviceLabel.appendChild(deviceInput);
  header.appendChild(deviceLabel);

  const settingsBtn = document.createElement('button');
  settingsBtn.textContent = '⚙ Settings';
  settingsBtn.title = 'Stream resolution, frame rate, bitrate, codec, audio';
  settingsBtn.style.marginLeft = 'auto';
  settingsBtn.onclick = () => openSettingsDialog();
  header.appendChild(settingsBtn);

  const addBtn = document.createElement('button');
  addBtn.textContent = '+ Add host';
  addBtn.style.marginLeft = '8px';
  addBtn.onclick = async () => {
    const host = prompt('Host address (IP or hostname):');
    if (!host) return;
    const hosts = loadHosts();
    hosts.push({
      id: crypto.randomUUID(),
      name: host,
      address: host,
      paired: false,
      lastSeen: Date.now(),
    });
    saveHosts(hosts);
    renderHosts();
  };
  header.appendChild(addBtn);

  const main = document.createElement('main');

  shell.appendChild(header);
  shell.appendChild(main);
  root.appendChild(shell);

  const hostList = new HostList(main, {
    onSelect: (host) => onHostClicked(host, caps, renderHosts),
  });

  function renderHosts() {
    hostList.render(loadHosts(), caps);
  }
  renderHosts();

  if (!caps.webCodecs) {
    const banner = document.createElement('div');
    banner.className = 'banner error';
    banner.textContent =
      'WebCodecs is not available in this browser. Video decoding will not work. ' +
      'Use Chrome/Edge 94+, Safari 16.4+ or Firefox 130+.';
    main.prepend(banner);
  } else if (!caps.crossOriginIsolated) {
    const banner = document.createElement('div');
    banner.className = 'banner warn';
    banner.textContent =
      'Page is not cross-origin isolated; WASM threading is disabled. Some features will run on the main thread.';
    main.prepend(banner);
  }
}

async function onHostClicked(host: Host, caps: Capabilities, refresh: () => void) {
  if (!host.paired) {
    const result = await openPairDialog(host);
    refresh();
    if (!result.paired) return;
    // Don't auto-stream after pair; let the user pick when to launch.
    return;
  }

  const app = await openAppPicker(host);
  if (!app) return;

  // Pick up settings fresh each click so changes made via the gear icon
  // take effect on the next stream without needing a reload.
  const settings = loadSettings();
  const streamConfig = toStreamConfig(settings);

  // Defer /launch until the wasm is ready (StreamView.start() calls this
  // thunk after client.prepare() succeeds). This keeps the gap between
  // /launch returning and the RTSP TCP connect short enough to fit inside
  // Sunshine's 10s ping_timeout.
  const launchFn = () => launchApp({ host, app, config: streamConfig });

  const view = new StreamView(document.body, host, caps, launchFn, { showStats: settings.showStats });
  try {
    await view.start(streamConfig);
  } catch (err) {
    alert(`Stream failed: ${(err as Error).message}`);
  }
}
