import { describe, expect, test } from 'bun:test';
import {
  collectMemSample,
  MEM_LOG_INTERVAL_MS,
  startMemLog,
  type MemClock,
  type MemSample,
} from './mem-log.js';

const usage = { rss: 133 * 1024 * 1024, heapUsed: 11, heapTotal: 22, external: 33 };

/** A clock the test advances by hand; no timers, no window, no native module. */
function manualClock(): {
  clock: MemClock;
  tick: () => void;
  interval: () => number;
  stopped: () => boolean;
} {
  let callback: (() => void) | null = null;
  let intervalMs = 0;
  return {
    clock: {
      setInterval: (fn, ms) => {
        callback = fn;
        intervalMs = ms;
        return 1;
      },
      clearInterval: () => {
        callback = null;
      },
    },
    tick: () => callback?.(),
    interval: () => intervalMs,
    stopped: () => callback === null,
  };
}

describe('collectMemSample', () => {
  test('sums replicaBytes over the mounted surfaces', () => {
    const sample = collectMemSample({
      memoryUsage: () => usage,
      now: () => 42,
      surfaceIds: () => [1, 2, 3],
      readStats: (id) => (id === 1 ? { replicaBytes: 100 } : id === 2 ? { replicaBytes: 2.5 } : {}),
    });
    expect(sample).toEqual({ at: 42, ...usage, replicaBytes: 102.5, surfaces: 3 });
  });

  test('a reader that throws or answers nothing is zero bytes, not a crash', () => {
    const sample = collectMemSample({
      memoryUsage: () => usage,
      surfaceIds: () => [7, 8],
      readStats: (id) => {
        if (id === 7) throw new Error('window gone');
        return null;
      },
    });
    expect(sample.replicaBytes).toBe(0);
    expect(sample.surfaces).toBe(2);
  });

  test('negative and non-finite bytes are ignored', () => {
    const sample = collectMemSample({
      memoryUsage: () => usage,
      surfaceIds: () => [1, 2],
      readStats: (id) => ({ replicaBytes: id === 1 ? -5 : Number.NaN }),
    });
    expect(sample.replicaBytes).toBe(0);
  });
});

describe('startMemLog', () => {
  test('samples immediately, then once per interval, and stops on demand', () => {
    const clock = manualClock();
    const samples: MemSample[] = [];
    const stop = startMemLog({
      clock: clock.clock,
      intervalMs: 5_000,
      log: (sample) => samples.push(sample),
      memoryUsage: () => usage,
      surfaceIds: () => [],
      readStats: () => null,
    });

    expect(samples.length).toBe(1);
    expect(clock.interval()).toBe(5_000);
    clock.tick();
    expect(samples.length).toBe(2);

    stop();
    expect(clock.stopped()).toBe(true);
    clock.tick();
    expect(samples.length).toBe(2);
  });

  test('defaults to the 30 s cadence B.2.1 asks for', () => {
    const clock = manualClock();
    const stop = startMemLog({
      clock: clock.clock,
      log: () => {},
      memoryUsage: () => usage,
      surfaceIds: () => [],
    });
    expect(MEM_LOG_INTERVAL_MS).toBe(30_000);
    expect(clock.interval()).toBe(MEM_LOG_INTERVAL_MS);
    stop();
  });
});
