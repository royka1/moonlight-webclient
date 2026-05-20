import { fetchAppList, type AppEntry } from '../client/nvhttp';
import type { Host } from '../client/host-store';

/**
 * Modal that fetches the host's app list and lets the user pick one.
 * Resolves with the chosen app, or null if cancelled.
 */
export function openAppPicker(host: Host): Promise<AppEntry | null> {
  return new Promise<AppEntry | null>((resolve) => {
    const scrim = document.createElement('div');
    scrim.className = 'modal-scrim';

    const modal = document.createElement('div');
    modal.className = 'modal';
    modal.style.minWidth = '420px';
    modal.style.maxHeight = '70vh';
    modal.style.overflow = 'hidden';

    const h2 = document.createElement('h2');
    h2.textContent = `Pick a game on ${host.name}`;
    modal.appendChild(h2);

    const status = document.createElement('p');
    status.textContent = 'Loading app list…';
    modal.appendChild(status);

    const list = document.createElement('div');
    list.style.overflowY = 'auto';
    list.style.maxHeight = '50vh';
    list.style.display = 'grid';
    list.style.gridTemplateColumns = 'repeat(auto-fill, minmax(140px, 1fr))';
    list.style.gap = '8px';
    list.style.padding = '4px 0';
    modal.appendChild(list);

    const actions = document.createElement('div');
    actions.className = 'actions';
    const cancelBtn = document.createElement('button');
    cancelBtn.textContent = 'Cancel';
    actions.appendChild(cancelBtn);
    modal.appendChild(actions);

    scrim.appendChild(modal);
    document.body.appendChild(scrim);

    let resolved = false;
    function finish(result: AppEntry | null) {
      if (resolved) return;
      resolved = true;
      scrim.remove();
      resolve(result);
    }

    cancelBtn.onclick = () => finish(null);

    fetchAppList(host)
      .then((apps) => {
        if (resolved) return;
        status.textContent = `${apps.length} app${apps.length === 1 ? '' : 's'} available.`;
        list.innerHTML = '';
        for (const app of apps) {
          const card = document.createElement('button');
          card.style.padding = '12px';
          card.style.fontSize = '13px';
          card.style.textAlign = 'left';
          card.style.whiteSpace = 'normal';
          card.style.lineHeight = '1.3';
          card.textContent = app.title;
          if (app.hdrSupported) {
            const hdr = document.createElement('div');
            hdr.textContent = 'HDR';
            hdr.style.fontSize = '10px';
            hdr.style.color = 'var(--accent)';
            hdr.style.marginTop = '4px';
            card.appendChild(hdr);
          }
          card.onclick = () => finish(app);
          list.appendChild(card);
        }
      })
      .catch((err: Error) => {
        if (resolved) return;
        status.textContent = `Failed to load: ${err.message}`;
        status.style.color = 'var(--danger)';
      });
  });
}
