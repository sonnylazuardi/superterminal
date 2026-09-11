/**
 * Reading the Window Placement from the renderer, kept free of `@gpuix/react`
 * imports so it is unit-testable without the native module.
 *
 * Like `readWindowSize`, a failed read is "no information", never a default:
 * when the native window is gone (closed, process on its way out)
 * `getWindowPlacement()` throws, and answering a made-up geometry there would
 * be dispatched and then persisted by the exit flush as the "last" placement.
 *
 * Fullscreen is deliberately not what gets stored: the next run restores a
 * maximized window instead (the handover's C-Q7), so `fullscreen: true`
 * becomes `maximized: true`.
 */

import type {
  WindowDisplay,
  WindowDisplayBounds,
  WindowPlacement,
} from './client-state.js';
import { MAX_WINDOW_DIMENSION } from './client-state.js';

/** The shape `GpuixRenderer.getWindowPlacement()` returns. */
export interface RendererWindowPlacement {
  x?: number;
  y?: number;
  width?: number;
  height?: number;
  maximized?: boolean;
  fullscreen?: boolean;
  display?: {
    uuid?: string | null;
    bounds?: {
      x?: number;
      y?: number;
      width?: number;
      height?: number;
    } | null;
  } | null;
}

export interface WindowPlacementSource {
  getWindowPlacement?: () => RendererWindowPlacement | null;
}

function finite(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function validSize(value: unknown, limit: number): number | undefined {
  const number = finite(value);
  return number !== undefined && number > 0 && number <= limit ? number : undefined;
}

function readDisplay(raw: RendererWindowPlacement['display']): WindowDisplay | undefined {
  if (!raw || typeof raw !== 'object') return undefined;
  const display: WindowDisplay = {};
  if (typeof raw.uuid === 'string' && raw.uuid.length > 0) display.uuid = raw.uuid;
  const x = finite(raw.bounds?.x);
  const y = finite(raw.bounds?.y);
  const width = validSize(raw.bounds?.width, MAX_WINDOW_DIMENSION);
  const height = validSize(raw.bounds?.height, MAX_WINDOW_DIMENSION);
  if (x !== undefined && y !== undefined && width !== undefined && height !== undefined) {
    const bounds: WindowDisplayBounds = { x, y, width, height };
    display.bounds = bounds;
  }
  return display.uuid !== undefined || display.bounds !== undefined ? display : undefined;
}

/**
 * The window's placement, or `null` when the renderer cannot answer.
 *
 * A missing origin or display degrades to a size-only placement; the reducer
 * then keeps whatever position it already had.
 */
export function readWindowPlacement(renderer: WindowPlacementSource | null): WindowPlacement | null {
  let placement: RendererWindowPlacement | null | undefined;
  try {
    placement = renderer?.getWindowPlacement?.();
  } catch {
    // The window is still opening or already destroyed.
    return null;
  }
  if (!placement || typeof placement !== 'object') return null;
  const width = validSize(placement.width, MAX_WINDOW_DIMENSION);
  const height = validSize(placement.height, MAX_WINDOW_DIMENSION);
  if (width === undefined || height === undefined) return null;

  const window: WindowPlacement = { width, height };
  const x = finite(placement.x);
  const y = finite(placement.y);
  if (x !== undefined && y !== undefined) {
    window.x = x;
    window.y = y;
  }
  // Never persist `fullscreen: true`: the next run opens maximized instead.
  window.maximized = placement.fullscreen === true || placement.maximized === true;
  const display = readDisplay(placement.display);
  if (display) window.display = display;
  return window;
}
