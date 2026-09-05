import { describe, expect, test } from 'bun:test';
import { readWindowSize } from './window-size.js';

describe('readWindowSize', () => {
  test('passes a real size through', () => {
    expect(readWindowSize({ getWindowSize: () => ({ width: 1200.8, height: 634.4 }) })).toEqual({
      width: 1200.8,
      height: 634.4,
    });
  });

  test('a destroyed window (throwing renderer) is "no information", not 800×600', () => {
    // gpuix's own useWindowSize answers 800×600 here; persisting that as the
    // last size is exactly the bug this guards against.
    const dead = {
      getWindowSize: () => {
        throw new Error('window not found');
      },
    };
    expect(readWindowSize(dead)).toBeNull();
  });

  test('no renderer, no method, or a zero size all read as null', () => {
    expect(readWindowSize(null)).toBeNull();
    expect(readWindowSize({})).toBeNull();
    expect(readWindowSize({ getWindowSize: () => ({ width: 0, height: 0 }) })).toBeNull();
  });
});
