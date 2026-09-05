/**
 * One active pointer drag for the whole window (Divider → sidebar width /
 * Split ratio).
 *
 * The Divider itself receives every event of a drag: gpuix gives an element
 * with both `onMouseDown` and `onMouseMove` GPUI pointer capture, so once the
 * band is pressed its moves and release keep arriving wherever the pointer
 * goes. Routing moves through an ancestor cannot work here — gpuix marks
 * every element with an opaque background as occluding (`should_occlude`),
 * so the frame/root are never "hovered" over a Pane or the sidebar. Verified
 * on Windows through GPUI-level dispatch (`util/drive.ts`).
 *
 * This module only keeps the drag's state and handlers between the press
 * and the release, so the Divider stays free of geometry.
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
