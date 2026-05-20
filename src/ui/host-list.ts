import type { Capabilities } from '../capabilities';
import type { Host } from '../client/host-store';

export interface HostListEvents {
  onSelect: (host: Host) => void;
}

export class HostList {
  private container: HTMLElement;

  constructor(parent: HTMLElement, private events: HostListEvents) {
    this.container = document.createElement('div');
    this.container.className = 'host-grid';
    parent.appendChild(this.container);
  }

  render(hosts: Host[], _caps: Capabilities) {
    this.container.innerHTML = '';
    if (hosts.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'banner';
      empty.textContent =
        'No hosts yet. Click "+ Add host" and enter the address of a machine running Sunshine or GeForce Experience.';
      this.container.appendChild(empty);
      return;
    }
    for (const host of hosts) {
      const card = document.createElement('div');
      card.className = 'host-card';
      card.onclick = () => this.events.onSelect(host);

      const h = document.createElement('h3');
      h.textContent = host.name;
      card.appendChild(h);

      const addr = document.createElement('div');
      addr.className = 'meta';
      addr.textContent = host.address;
      card.appendChild(addr);

      const status = document.createElement('div');
      status.className = 'meta';
      status.textContent = host.paired ? 'Paired' : 'Not paired - click to pair';
      card.appendChild(status);

      this.container.appendChild(card);
    }
  }
}
