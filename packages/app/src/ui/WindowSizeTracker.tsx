/**
 * Keeps `ui.window` in the store equal to the real window size.
 *
 * gpuix emits no resize event (nothing in `@gpuix/native` produces a
 * `windowResize` payload), so the size has to be sampled, the way gpuix's
 * own `useWindowSize` does. That hook is not used on purpose: it answers a
 * hard-coded 800×600 once the native window is gone — see `readWindowSize`.
 *
 * This is the single writer of `window.resize`; everything else (Dialog
 * placement, Client State) reads the store. Renders nothing.
 */

import { useGpuix } from '@gpuix/react';
import { useEffect } from 'react';
import { readWindowSize } from '../state/window-size.js';
import { useServices } from './context.js';

/** 100 ms is gpuix's own default; a drag still lands within a frame or two. */
const SAMPLE_MS = 100;

export function WindowSizeTracker() {
  const { store } = useServices();
  const { renderer } = useGpuix();

  useEffect(() => {
    const sample = () => {
      const size = readWindowSize(renderer);
      if (size) store.dispatch({ type: 'window.resize', ...size });
    };
    sample();
    const timer = setInterval(sample, SAMPLE_MS);
    return () => clearInterval(timer);
  }, [store, renderer]);

  return null;
}
