/**
 * Dev-only input driver: `SUPERTERMINAL_DRIVE=<file>` makes the client tail
 * that file and replay each new line through gpuix's in-process
 * `simulate*` renderer API, i.e. through GPUI's own hit-testing and dispatch
 * — the path real input takes — without any OS-level input synthesis.
 *
 * Why this exists: Win32 `PostMessage`/`SendInput` synthesis leaves GPUI's
 * Windows backend with stuck mouse capture and never produced `mouseMove`
 * events, so drags could not be exercised from a script at all.
 *
 * Lines (logical px, window coordinates):
 *   down X Y [button]      move X Y [pressedButton]     up X Y [button]
 *   click X Y [button]     key <keystroke>              keys <text>
 *   bounds ID              logs the element's last painted bounds
 * Inactive unless the env var is set; never loaded into the render path.
 */

import { readFileSync, watchFile } from 'node:fs';
import { debug } from './debug.js';

const log = debug('st:drive');

interface SimulatingRenderer {
  simulateMouseDown?: (x: number, y: number, button?: number) => void;
  simulateMouseUp?: (x: number, y: number, button?: number) => void;
  simulateMouseMove?: (x: number, y: number, pressedButton?: number) => void;
  simulateClick?: (x: number, y: number, button?: number) => void;
  simulateKeyDown?: (keystroke: string) => void;
  simulateKeystrokes?: (text: string) => void;
  getElementBounds?: (id: number) => number[] | null;
}

export function startDriveFile(path: string, renderer: SimulatingRenderer): void {
  let consumed = 0;
  const run = (line: string): void => {
    const [cmd, a, b, c] = line.trim().split(/\s+/);
    const x = Number(a);
    const y = Number(b);
    const n = c === undefined ? undefined : Number(c);
    log('run', line.trim());
    try {
      switch (cmd) {
        case 'down':
          renderer.simulateMouseDown?.(x, y, n);
          break;
        case 'up':
          renderer.simulateMouseUp?.(x, y, n);
          break;
        case 'move':
          renderer.simulateMouseMove?.(x, y, n);
          break;
        case 'click':
          renderer.simulateClick?.(x, y, n);
          break;
        case 'key':
          renderer.simulateKeyDown?.(a ?? '');
          break;
        case 'keys':
          renderer.simulateKeystrokes?.(line.trim().slice(5));
          break;
        case 'bounds':
          log('bounds', x, JSON.stringify(renderer.getElementBounds?.(x) ?? null));
          break;
        default:
          log('unknown command', cmd);
      }
    } catch (err) {
      log('failed', String(err));
    }
  };
  const poll = (): void => {
    let text: string;
    try {
      text = readFileSync(path, 'utf8');
    } catch {
      return;
    }
    if (text.length <= consumed) return;
    const fresh = text.slice(consumed);
    const end = fresh.lastIndexOf('\n');
    if (end < 0) return;
    consumed += end + 1;
    for (const line of fresh.slice(0, end).split('\n')) if (line.trim()) run(line);
  };
  watchFile(path, { interval: 50 }, poll);
  log('driving from', path);
}
