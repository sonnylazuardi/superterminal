/**
 * Reading the window size from the renderer, kept free of `@gpuix/react`
 * imports so it is unit-testable without the native module.
 *
 * A failed read is "no information", never a default: when the native window
 * is gone (closed, process on its way out) `getWindowSize()` throws, and
 * answering 800×600 there — as gpuix's own `useWindowSize` does — would be
 * dispatched and then persisted by the exit flush as the "last" size.
 */

export interface WindowSizeSource {
  getWindowSize?: () => { width: number; height: number };
}

/** The window's size, or `null` when the renderer cannot answer. */
export function readWindowSize(
  renderer: WindowSizeSource | null,
): { width: number; height: number } | null {
  try {
    const size = renderer?.getWindowSize?.();
    if (size && size.width > 0 && size.height > 0) return { width: size.width, height: size.height };
  } catch {
    // The window is still opening or already destroyed.
  }
  return null;
}
