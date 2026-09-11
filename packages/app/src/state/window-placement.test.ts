import { describe, expect, test } from 'bun:test';
import { readWindowPlacement, type RendererWindowPlacement } from './window-placement.js';

/** What the patched gpuix returns on a two-monitor machine. */
const base: RendererWindowPlacement = {
  x: 120.5,
  y: 64,
  width: 1200,
  height: 800,
  maximized: false,
  fullscreen: false,
  display: {
    uuid: 'DISPLAY-UUID',
    bounds: { x: 0, y: 0, width: 2560, height: 1440 },
  },
};

const from = (placement: unknown) => ({
  getWindowPlacement: () => placement as RendererWindowPlacement,
});

describe('readWindowPlacement', () => {
  test('passes a real placement through', () => {
    expect(readWindowPlacement(from(base))).toEqual({
      width: 1200,
      height: 800,
      x: 120.5,
      y: 64,
      maximized: false,
      display: {
        uuid: 'DISPLAY-UUID',
        bounds: { x: 0, y: 0, width: 2560, height: 1440 },
      },
    });
  });

  test('a destroyed window (throwing renderer) is "no information", not a default', () => {
    // A made-up geometry here would be persisted by the exit flush as the
    // "last" placement, exactly the bug `readWindowSize` guards against.
    const dead = {
      getWindowPlacement: () => {
        throw new Error('window not found');
      },
    };
    expect(readWindowPlacement(dead)).toBeNull();
  });

  test('no renderer, no method, or no size all read as null', () => {
    expect(readWindowPlacement(null)).toBeNull();
    expect(readWindowPlacement({})).toBeNull();
    expect(readWindowPlacement(from(null))).toBeNull();
    expect(readWindowPlacement(from({ ...base, width: 0 }))).toBeNull();
    expect(readWindowPlacement(from({ ...base, height: Number.NaN }))).toBeNull();
    expect(readWindowPlacement(from({ ...base, width: 99_999 }))).toBeNull();
  });

  test('fullscreen is stored as maximized, never as fullscreen', () => {
    const placement = readWindowPlacement(from({ ...base, fullscreen: true }));
    expect(placement?.maximized).toBe(true);
    expect(placement).not.toHaveProperty('fullscreen');
  });

  test('a missing origin or display degrades to a size-only placement', () => {
    const placement = readWindowPlacement(
      from({ width: 1200, height: 800, maximized: false, fullscreen: false }),
    );
    expect(placement).toEqual({ width: 1200, height: 800, maximized: false });
  });

  test('a display uuid survives without bounds, and junk fields are dropped', () => {
    const uuidOnly = readWindowPlacement(
      from({ ...base, display: { uuid: 'U', bounds: { x: 'left', width: 10 } } }),
    );
    expect(uuidOnly?.display).toEqual({ uuid: 'U' });
    const junk = readWindowPlacement(from({ ...base, display: { uuid: 7, bounds: 'nope' } }));
    expect(junk).not.toHaveProperty('display');
  });
});
