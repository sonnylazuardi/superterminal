import { describe, expect, test } from 'bun:test';
import { glassTokens } from '../theme/tokens.js';
import { REORDER_THRESHOLD, dropIndex, moveItem, previewOrder, tabExtent } from './tab-drag.js';

describe('tabExtent', () => {
  test('vertical rows pitch at rowHeight plus the row margin', () => {
    expect(tabExtent(true, glassTokens)).toBe(glassTokens.strip.rowHeight + glassTokens.space.xs);
  });

  test('horizontal tabs pitch at their max width plus the strip gap', () => {
    expect(tabExtent(false, glassTokens)).toBe(glassTokens.strip.tabMaxWidth + glassTokens.strip.gap);
  });

  test('the threshold is a few pixels, not zero', () => {
    expect(REORDER_THRESHOLD).toBeGreaterThan(0);
    expect(REORDER_THRESHOLD).toBeLessThan(10);
  });
});

describe('dropIndex', () => {
  const extent = 32;

  test('less than half a slot of travel keeps the tab where it is', () => {
    expect(dropIndex(2, 15, extent, 5)).toBe(2);
    expect(dropIndex(2, -15, extent, 5)).toBe(2);
  });

  test('more than half a slot swaps with the neighbour', () => {
    expect(dropIndex(2, 17, extent, 5)).toBe(3);
    expect(dropIndex(2, -17, extent, 5)).toBe(1);
  });

  test('rounds to the nearest slot over long drags', () => {
    expect(dropIndex(0, 32 * 2.4, extent, 5)).toBe(2);
    expect(dropIndex(0, 32 * 2.6, extent, 5)).toBe(3);
  });

  test('clamps to the strip', () => {
    expect(dropIndex(4, 1000, extent, 5)).toBe(4);
    expect(dropIndex(0, -1000, extent, 5)).toBe(0);
    expect(dropIndex(3, -1000, extent, 5)).toBe(0);
  });

  test('is inert on a degenerate extent or delta', () => {
    expect(dropIndex(2, 100, 0, 5)).toBe(2);
    expect(dropIndex(2, Number.NaN, extent, 5)).toBe(2);
    expect(dropIndex(2, 100, extent, 0)).toBe(0);
  });
});

describe('moveItem', () => {
  const items = ['a', 'b', 'c', 'd'] as const;

  test('moves forward and backward', () => {
    expect(moveItem(items, 0, 2)).toEqual(['b', 'c', 'a', 'd']);
    expect(moveItem(items, 3, 1)).toEqual(['a', 'd', 'b', 'c']);
  });

  test('returns the same array for a no-op so subscribers do not re-render', () => {
    expect(moveItem(items, 1, 1)).toBe(items);
    expect(moveItem(items, -1, 2)).toBe(items);
    expect(moveItem(items, 1, 4)).toBe(items);
  });

  test('does not mutate its input', () => {
    const copy = [...items];
    moveItem(copy, 0, 3);
    expect(copy).toEqual([...items]);
  });
});

describe('previewOrder', () => {
  const ids = [10, 11, 12, 13];

  test('no drag is the identity', () => {
    expect(previewOrder(ids, null)).toBe(ids);
  });

  test('moves the dragged tab to its slot', () => {
    expect(previewOrder(ids, { tabId: 10, to: 2 })).toEqual([11, 12, 10, 13]);
    expect(previewOrder(ids, { tabId: 13, to: 0 })).toEqual([13, 10, 11, 12]);
  });

  test('re-finds the tab by id rather than trusting a stale index', () => {
    // The server reordered under us: 12 now sits first.
    const reordered = [12, 10, 11, 13];
    expect(previewOrder(reordered, { tabId: 12, to: 3 })).toEqual([10, 11, 13, 12]);
  });

  test('a tab that closed mid-drag previews nothing', () => {
    expect(previewOrder(ids, { tabId: 99, to: 1 })).toBe(ids);
  });

  test('a slot past the end lands on the last tab', () => {
    expect(previewOrder(ids, { tabId: 10, to: 42 })).toEqual([11, 12, 13, 10]);
  });

  test('a slot equal to the current index is the identity', () => {
    expect(previewOrder(ids, { tabId: 11, to: 1 })).toBe(ids);
  });
});
