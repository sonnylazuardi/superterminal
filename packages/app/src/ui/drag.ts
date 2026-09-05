/**
 * One active pointer drag for the whole window (Divider → sidebar width /
 * Split ratio).
 *
 * Why the moves are routed through the app root instead of the Divider's
 * own `onMouseMove`: gpuix calls GPUI `capture_pointer()` on any element
 * that has BOTH `onMouseDown` and `onMouseMove`, and in the vendored GPUI a
 * captured hitbox id is re-mapped on the next frame — the press turns into
 * a click on whichever element inherits that id (observed on Windows: the
 * session chip) and the Divider never sees its move/up. With only
 * `onMouseDown` on the Divider the press is delivered correctly, so the
 * Divider registers a drag here and `App` attaches `onMouseMove`/`onMouseUp`
 * to the root only while one is active.
 *
 * A tiny external store so React can subscribe to "is a drag active".
 */

export interface DragHandlers {
  /** Called with the pointer position along the drag axis. */
  onMove: (pos: number) => void;
  onEnd: () => void;
}

export interface DragEventLike {
  x?: number;
  y?: number;
  /** gpuix sets this on `mouseMove`; a released primary button ends the drag. */
  pressedButton?: number;
}

const PRIMARY = 0;

let active: { axis: 'row' | 'column'; handlers: DragHandlers } | null = null;
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of [...listeners]) listener();
}

export const dragController = {
  begin(axis: 'row' | 'column', handlers: DragHandlers): void {
    if (active) active.handlers.onEnd();
    active = { axis, handlers };
    notify();
  },
  move(event: DragEventLike): void {
    if (!active) return;
    if (event.pressedButton !== undefined && event.pressedButton !== PRIMARY) {
      // The button went up somewhere we did not see (focus change, etc.).
      dragController.end();
      return;
    }
    const pos = active.axis === 'row' ? event.x : event.y;
    if (typeof pos === 'number' && Number.isFinite(pos)) active.handlers.onMove(pos);
  },
  end(): void {
    if (!active) return;
    const { handlers } = active;
    active = null;
    notify();
    handlers.onEnd();
  },
  isActive(): boolean {
    return active !== null;
  },
  /** `useSyncExternalStore` subscription. */
  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
};
