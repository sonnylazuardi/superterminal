/**
 * `DEBUG=st:mem` memory sampling (handover Part B, B.2.1).
 *
 * Every [`MEM_LOG_INTERVAL_MS`] this writes one JSON line to stderr:
 *
 *   st:mem {"at":…,"rss":…,"heapUsed":…,"heapTotal":…,"external":…,"replicaBytes":…,"surfaces":…}
 *
 * All sizes are bytes. The four process fields come straight from
 * `process.memoryUsage()`; `replicaBytes` is the sum of
 * `stReadProp(surfaceId, 'stats').replicaBytes` over every mounted Grid, the
 * number `docs/perf/memory-2026-09.md` pairs with the PowerShell sampler.
 *
 * Everything that touches the outside world is injectable — the clock, the
 * logger, the mounted Surface ids and the stats reader — so the test drives it
 * with no window, no timer and no native module. The one-line integration for
 * `app.tsx` is `import './debug/mem-log.js';`: importing is enough, the sampler
 * starts itself only when `DEBUG` enables `st:mem` (e.g. `st:*`).
 */

import { debug } from '../util/debug.js';

/** The four `process.memoryUsage()` fields the baseline tracks. Bytes. */
export interface MemoryUsage {
  rss: number;
  heapUsed: number;
  heapTotal: number;
  external: number;
}

/** The slice of the native `stats` prop this sampler needs. */
export interface SurfaceStats {
  replicaBytes?: number;
}

/** One sample: process memory plus every mounted Replica. Bytes. */
export interface MemSample extends MemoryUsage {
  /** `Date.now()` when the sample was taken, for matching the CSV sampler. */
  at: number;
  /** Sum of `stats.replicaBytes` over the mounted Surfaces. */
  replicaBytes: number;
  /** How many mounted Surfaces the sum covered. */
  surfaces: number;
}

/** The timer surface, injectable so the test owns the ticks. */
export interface MemClock {
  setInterval: (callback: () => void, ms: number) => unknown;
  clearInterval: (handle: unknown) => void;
}

export interface MemLogDeps {
  /** Receives one sample per tick. Required — logging is the whole point. */
  log: (sample: MemSample) => void;
  /** Tick period; defaults to 30 s. */
  intervalMs?: number;
  /** Defaults to the global timers. */
  clock?: MemClock;
  /** Defaults to `process.memoryUsage()`. */
  memoryUsage?: () => MemoryUsage;
  /** Defaults to the native `stListGrids()`. */
  surfaceIds?: () => number[];
  /** Defaults to the native `stReadProp(id, 'stats')`. */
  readStats?: (surfaceId: number) => SurfaceStats | null | undefined;
  /** Defaults to `Date.now`. */
  now?: () => number;
}

/** 30 s, the cadence B.2.1 asks for. */
export const MEM_LOG_INTERVAL_MS = 30_000;

const defaultClock: MemClock = {
  setInterval: (callback, ms) => setInterval(callback, ms),
  clearInterval: (handle) => clearInterval(handle as ReturnType<typeof setInterval>),
};

/** Reads one sample; schedules nothing. */
export function collectMemSample(
  deps: Pick<MemLogDeps, 'memoryUsage' | 'surfaceIds' | 'readStats' | 'now'>,
): MemSample {
  const usage = (deps.memoryUsage ?? (() => process.memoryUsage()))();
  const ids = (deps.surfaceIds ?? nativeSurfaceIds)();
  const readStats = deps.readStats ?? nativeStats;

  let replicaBytes = 0;
  let surfaces = 0;
  for (const id of ids) {
    surfaces += 1;
    try {
      const bytes = readStats(id)?.replicaBytes;
      // A missing `replicaBytes` (native build older than B.2 step 2) or a
      // surface mid-teardown is "no data", not a reason to stop sampling.
      if (typeof bytes === 'number' && Number.isFinite(bytes) && bytes > 0) replicaBytes += bytes;
    } catch {
      // Keep the process numbers even when one surface cannot answer.
    }
  }

  return {
    at: (deps.now ?? Date.now)(),
    rss: usage.rss,
    heapUsed: usage.heapUsed,
    heapTotal: usage.heapTotal,
    external: usage.external,
    replicaBytes,
    surfaces,
  };
}

/**
 * Logs one sample immediately, then every `intervalMs`. Returns the stop
 * function. The immediate sample is the launch measurement the baseline needs.
 */
export function startMemLog(deps: MemLogDeps): () => void {
  const clock = deps.clock ?? defaultClock;
  const intervalMs = deps.intervalMs ?? MEM_LOG_INTERVAL_MS;
  const tick = (): void => {
    deps.log(collectMemSample(deps));
  };
  tick();
  const handle = clock.setInterval(tick, intervalMs);
  return () => clock.clearInterval(handle);
}

// --- native readers ---------------------------------------------------------

interface NativeModule {
  stListGrids?: () => unknown;
  stReadProp?: (surfaceId: number, key: string) => unknown;
}

let native: NativeModule | null = null;

/**
 * The `.node` the preload already pointed `NAPI_RS_NATIVE_LIBRARY_PATH` at.
 * `require` returns the cached addon, so this cannot create a second GPUI
 * instance; a missing build or a not-yet-set env var is just `null`.
 */
function loadNative(): NativeModule | null {
  if (native) return native;
  try {
    const path = process.env['NAPI_RS_NATIVE_LIBRARY_PATH'];
    if (!path) return null;
    const mod = require(path) as NativeModule;
    if (typeof mod?.stListGrids !== 'function' || typeof mod?.stReadProp !== 'function') return null;
    native = mod;
    return mod;
  } catch {
    return null;
  }
}

function nativeSurfaceIds(): number[] {
  try {
    const ids = loadNative()?.stListGrids?.();
    return Array.isArray(ids) ? ids.filter((id): id is number => typeof id === 'number') : [];
  } catch {
    return [];
  }
}

function nativeStats(surfaceId: number): SurfaceStats | null {
  try {
    const stats = loadNative()?.stReadProp?.(surfaceId, 'stats');
    if (stats && typeof stats === 'object' && !Array.isArray(stats)) {
      const bytes = (stats as { replicaBytes?: unknown }).replicaBytes;
      return typeof bytes === 'number' ? { replicaBytes: bytes } : {};
    }
  } catch {
    // No window yet, or the element retired mid-read.
  }
  return null;
}

// One-line integration for app.tsx (do this after `import './native/preload.js'`):
//
//   import './debug/mem-log.js';
//
// The sampler starts itself only when DEBUG enables `st:mem`.
const memLog = debug('st:mem');
if (memLog.enabled) {
  startMemLog({
    log: (sample) => memLog(JSON.stringify(sample)),
  });
}
