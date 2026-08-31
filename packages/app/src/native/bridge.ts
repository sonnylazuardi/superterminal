/**
 * Adapter between the command registry and the native `<terminal-grid>`.
 *
 * Imperative writes that are not state (`copy`, `paste`, `clearScrollback`) are
 * modelled as a monotonically increasing `command: {seq, name, args}` prop —
 * the standard trick for one-shot commands in a retained tree (04 §3). This
 * bus holds the latest command and lets `<SurfaceHost>` subscribe to it with
 * `useSyncExternalStore`, so no component keeps mutable state of its own.
 *
 * BLOCKED ON 04: reading back (`selectionText`, `scrollOffset`) needs the
 * `get_custom_prop` napi addition that ships with the factory-hook patch. Until
 * then `copySelection` issues the command — the element writes the clipboard
 * itself (04 §9) — and reports success without the text.
 */

import type { NativeBridge } from '../commands/types.js';

export interface GridCommand {
  surfaceId: number;
  seq: number;
  name: 'copy' | 'paste' | 'scrollToBottom' | 'clearScrollback';
  args?: unknown;
}

export interface NativeCommandBus {
  subscribe(listener: () => void): () => void;
  getSnapshot(): GridCommand | null;
  send(surfaceId: number, name: GridCommand['name'], args?: unknown): void;
}

export function createCommandBus(): NativeCommandBus {
  let current: GridCommand | null = null;
  let seq = 0;
  const listeners = new Set<() => void>();
  return {
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    getSnapshot: () => current,
    send(surfaceId, name, args) {
      seq += 1;
      current = args === undefined ? { surfaceId, seq, name } : { surfaceId, seq, name, args };
      for (const listener of [...listeners]) listener();
    },
  };
}

export interface NativeBridgeDeps {
  bus: NativeCommandBus;
  /** `renderer.getSelectedText()` once gpuix exposes it; optional today. */
  readSelection?: (surfaceId: number) => string | null;
  focusElement?: (surfaceId: number) => void;
}

export function createNativeBridge(deps: NativeBridgeDeps): NativeBridge {
  return {
    hasSelection(surfaceId) {
      return deps.readSelection ? deps.readSelection(surfaceId) !== null : false;
    },
    copySelection(surfaceId) {
      deps.bus.send(surfaceId, 'copy');
      // The element owns the clipboard write; '' means "done, text unknown".
      return deps.readSelection?.(surfaceId) ?? '';
    },
    paste(surfaceId) {
      deps.bus.send(surfaceId, 'paste');
    },
    clearScrollback(surfaceId) {
      deps.bus.send(surfaceId, 'clearScrollback');
    },
    focus(surfaceId) {
      deps.focusElement?.(surfaceId);
    },
  };
}
