import type { Host } from '../client/host-store';
import { pairHost } from '../client/pairing';
import { upsertHost } from '../client/host-store';

export interface PairDialogResult {
  paired: boolean;
  host: Host;
}

/**
 * Modal that generates a 4-digit PIN, immediately fires the pairing request
 * at the host (so Sunshine pops its PIN prompt and *waits* for the user),
 * and resolves when pairing completes, the user cancels, or it errors.
 *
 * The previous implementation showed a "Continue" button between the PIN
 * display and the network call, on the mistaken assumption that the host
 * only accepts the pair request after the PIN has been entered. The
 * opposite is true: Sunshine accepts the request first and uses it to
 * pop the PIN prompt on its own UI. Other Moonlight clients work that way
 * too, which is why they don't need a "Continue" step.
 */
export function openPairDialog(host: Host): Promise<PairDialogResult> {
  return new Promise<PairDialogResult>((resolve) => {
    const pin = generatePin();

    const scrim = document.createElement('div');
    scrim.className = 'modal-scrim';

    const modal = document.createElement('div');
    modal.className = 'modal';

    const h2 = document.createElement('h2');
    h2.textContent = `Pair with ${host.name}`;
    modal.appendChild(h2);

    const intro = document.createElement('p');
    intro.innerHTML =
      'Open Sunshine\'s Web UI on the host and enter this PIN when prompted, ' +
      'then click <em>Submit</em>. Take your time — Sunshine waits for the PIN.';
    modal.appendChild(intro);

    const pinEl = document.createElement('div');
    pinEl.className = 'pin';
    pinEl.textContent = pin;
    modal.appendChild(pinEl);

    const status = document.createElement('div');
    status.className = 'status';
    status.textContent = 'Waiting for PIN entry on host…';
    modal.appendChild(status);

    const actions = document.createElement('div');
    actions.className = 'actions';

    const cancelBtn = document.createElement('button');
    cancelBtn.textContent = 'Cancel';
    actions.appendChild(cancelBtn);

    modal.appendChild(actions);
    scrim.appendChild(modal);
    document.body.appendChild(scrim);

    let aborted = false;
    const ac = new AbortController();
    cancelBtn.onclick = () => {
      aborted = true;
      ac.abort();
      cleanup();
      resolve({ paired: false, host });
    };

    function cleanup() {
      scrim.remove();
    }

    // Fire the handshake immediately — Sunshine's step-1 request is what
    // makes the PIN dialog appear on the host, and that endpoint blocks
    // until the user submits the PIN. So sending it now is what gives the
    // user the rest of their attention budget to enter the PIN.
    pairHost(host, pin, {
      signal: ac.signal,
      onProgress: (p) => {
        if (aborted) return;
        status.textContent = p.message;
      },
    })
      .then((paired) => {
        if (aborted) return;
        status.textContent = 'Paired ✔';
        status.classList.remove('error');
        upsertHost(paired);
        cancelBtn.textContent = 'Done';
        cancelBtn.classList.add('primary');
        cancelBtn.onclick = () => {
          cleanup();
          resolve({ paired: true, host: paired });
        };
        setTimeout(() => {
          if (document.body.contains(scrim)) {
            cleanup();
            resolve({ paired: true, host: paired });
          }
        }, 1200);
      })
      .catch((err: Error) => {
        if (aborted) return;
        status.textContent = `Pairing failed: ${err.message}`;
        status.classList.add('error');
      });
  });
}

function generatePin(): string {
  // Cryptographically random 4-digit PIN; matches the format both
  // Sunshine and GFE accept.
  const buf = new Uint32Array(1);
  crypto.getRandomValues(buf);
  return String(buf[0] % 10000).padStart(4, '0');
}
