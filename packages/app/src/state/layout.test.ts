import { describe, expect, test } from 'bun:test';
import type { Layout } from '@superterminal/protocol-ts';
import {
  clampRatio,
  clampSidebarWidth,
  contentRect,
  DIVIDER_GRAB,
  dragPosition,
  dragRatio,
  firstLeaf,
  focusedSurfaceOf,
  menuItemsFor,
  pathKey,
  relativeLeaf,
  siblingLeaf,
  splitAt,
  splitGeometry,
} from './layout.js';

const leaf = (surface: number): Layout => ({ kind: 'leaf', surface });
const row = (ratio: number, first: Layout, second: Layout): Layout => ({
  kind: 'split',
  axis: 'row',
  ratio,
  first,
  second,
});
const column = (ratio: number, first: Layout, second: Layout): Layout => ({
  kind: 'split',
  axis: 'column',
  ratio,
  first,
  second,
});

/** [1 | [2 / 3]] */
const TREE = row(0.5, leaf(1), column(0.25, leaf(2), leaf(3)));

describe('tree helpers', () => {
  test('splitAt walks 0 = first, 1 = second and rejects leaves', () => {
    expect(splitAt(TREE, [])!.axis).toBe('row');
    expect(splitAt(TREE, [1])!.axis).toBe('column');
    expect(splitAt(TREE, [0])).toBeNull();
    expect(splitAt(TREE, [1, 0])).toBeNull();
    expect(splitAt(TREE, [1, 1, 1])).toBeNull();
    expect(splitAt(leaf(9), [])).toBeNull();
  });

  test('firstLeaf and siblingLeaf', () => {
    expect(firstLeaf(TREE)).toBe(1);
    expect(firstLeaf(leaf(7))).toBe(7);
    expect(siblingLeaf(TREE, 1)).toBe(2); // sibling subtree [2/3] → its first leaf
    expect(siblingLeaf(TREE, 2)).toBe(3);
    expect(siblingLeaf(TREE, 3)).toBe(2);
    expect(siblingLeaf(TREE, 99)).toBeNull();
    expect(siblingLeaf(leaf(1), 1)).toBeNull(); // the only Pane: the Tab closes
  });

  test('relativeLeaf wraps and tolerates an unknown current', () => {
    expect(relativeLeaf([1, 2, 3], 3, 1)).toBe(1);
    expect(relativeLeaf([1, 2, 3], 1, -1)).toBe(3);
    expect(relativeLeaf([1, 2, 3], 99, 1)).toBe(2);
    expect(relativeLeaf([], 1, 1)).toBeNull();
  });

  test('focusedSurfaceOf keeps a valid choice and falls back to the first Pane', () => {
    const tab = { surfaceId: 1, surfaceIds: [1, 2, 3] };
    expect(focusedSurfaceOf(tab, 3)).toBe(3);
    expect(focusedSurfaceOf(tab, 9)).toBe(1);
    expect(focusedSurfaceOf(tab, undefined)).toBe(1);
    expect(focusedSurfaceOf({ surfaceId: 5, surfaceIds: [] }, undefined)).toBe(5);
  });

  test('pathKey', () => {
    expect(pathKey([])).toBe('');
    expect(pathKey([1, 0])).toBe('1/0');
  });
});

describe('clamps', () => {
  test('sidebar width', () => {
    expect(clampSidebarWidth(200.4)).toBe(200);
    expect(clampSidebarWidth(1)).toBe(160);
    expect(clampSidebarWidth(9000)).toBe(480);
    expect(clampSidebarWidth(Number.NaN)).toBe(160);
  });

  test('ratio', () => {
    expect(clampRatio(0.5)).toBe(0.5);
    expect(clampRatio(0)).toBe(0.1);
    expect(clampRatio(1)).toBe(0.9);
    expect(clampRatio(Number.NaN)).toBe(0.5);
  });
});

describe('geometry', () => {
  const rect = { x: 100, y: 38, width: 1007, height: 600 };

  test('content rect subtracts the chrome for both layouts', () => {
    const tokens = { strip: { titleBarHeight: 38, height: 36 } };
    expect(contentRect({ width: 1200, height: 800 }, true, 220, tokens)).toEqual({
      x: 220 + DIVIDER_GRAB,
      y: 38,
      width: 1200 - 220 - DIVIDER_GRAB,
      height: 762,
    });
    expect(contentRect({ width: 1200, height: 800 }, false, 220, tokens)).toEqual({
      x: 0,
      y: 74,
      width: 1200,
      height: 726,
    });
    // An unmeasured window never yields a negative rect.
    expect(contentRect({ width: 0, height: 0 }, true, 220, tokens)).toMatchObject({ width: 0, height: 0 });
  });

  test('nested splits hand their children ratio / 1-ratio of the free extent', () => {
    const g = splitGeometry(TREE, rect, DIVIDER_GRAB);
    expect(g.get('')).toEqual({ axis: 'row', extent: 1007, rect });
    // Second child of the root row: x moves past first (500) + divider.
    const free = 1007 - DIVIDER_GRAB;
    expect(g.get('1')).toEqual({
      axis: 'column',
      extent: 600,
      rect: { x: 100 + free * 0.5 + DIVIDER_GRAB, y: 38, width: free * 0.5, height: 600 },
    });
    expect(g.size).toBe(2); // leaves have no geometry
  });

  test('a preview ratio reshapes the subtree under it', () => {
    const g = splitGeometry(TREE, rect, DIVIDER_GRAB, { path: [], ratio: 0.2 });
    const free = 1007 - DIVIDER_GRAB;
    expect(g.get('1')!.rect.width).toBeCloseTo(free * 0.8);
    // A preview for another path leaves the root alone.
    const other = splitGeometry(TREE, rect, DIVIDER_GRAB, { path: [1], ratio: 0.9 });
    expect(other.get('1')!.rect.width).toBeCloseTo(free * 0.5);
  });

  test('dragRatio is delta-based and clamped; a zero extent is inert', () => {
    expect(dragRatio(0.5, 100, 200, 1000)).toBeCloseTo(0.6);
    expect(dragRatio(0.5, 200, 100, 1000)).toBeCloseTo(0.4);
    expect(dragRatio(0.5, 0, 5000, 1000)).toBe(0.9);
    expect(dragRatio(0.5, 0, 50, 0)).toBe(0.5);
  });

  test('dragPosition picks the axis coordinate and rejects a missing one', () => {
    expect(dragPosition('row', { x: 12, y: 34 })).toBe(12);
    expect(dragPosition('column', { x: 12, y: 34 })).toBe(34);
    expect(dragPosition('row', { y: 34 })).toBeNull();
    expect(dragPosition('column', { x: 1, y: Number.NaN })).toBeNull();
  });
});

describe('menuItemsFor', () => {
  test('offers Close Pane only for a split Tab, Close Tab always last', () => {
    expect(menuItemsFor({ surfaceIds: [1] }).map((i) => i.commandId)).toEqual([
      'pane.splitRight',
      'pane.splitDown',
      'tab.close',
    ]);
    expect(menuItemsFor({ surfaceIds: [1, 2] }).map((i) => i.commandId)).toEqual([
      'pane.splitRight',
      'pane.splitDown',
      'pane.close',
      'tab.close',
    ]);
    expect(menuItemsFor(undefined)).toEqual([]);
  });
});
