/**
 * Reordering tabs by dragging (05 §4): the pure half.
 *
 * The wire has had `tab.reorder { tab, index }` since 1.0 and the server
 * implements it; only the UI was missing. Nothing here touches the protocol
 * or React — `TabStrip.tsx` owns the pointer plumbing and the commit.
 *
 * # Why the drag is delta-based
 *
 * The same constraint `layout.ts` documents for dividers: gpuix reports
 * pointer positions in window coordinates but exposes no element bounds to
 * JavaScript (`getElementBounds` is the testing surface, and
 * `automation::track_own_bounds` is applied only to the native grid). So a
 * tab cannot ask where it or its neighbours are.
 *
 * It does not need to. A reorder only cares how far the pointer has travelled
 * *since the press*, in slots: `round(delta / extent)`. The origin never
 * enters, so the banner, the session chip and the sidebar header cannot skew
 * it. `round`, not `trunc`, so a tab swaps once it is more than halfway across
 * its neighbour.
 *
 * # The extents, and the one approximation
 *
 * Vertical is exact: every row is `rowHeight + space.xs` (its `marginBottom`),
 * both tokens, both set by `Tab`. Horizontal uses `tabMaxWidth + gap`: tabs
 * there are content-sized between `tabMinWidth` and `tabMaxWidth`, and text
 * width is not knowable in JS. That is exact once titles are long enough to
 * ellipsize — the crowded case. With short titles the real tabs are narrower,
 * so the drag reads slightly low: the pointer travels a little further than
 * the tabs it passes. It shows as drag *gain*, never a wrong drop, because the
 * user steers by the live preview and releases when the gap is where they
 * want it. If gpuix ever exposes bounds to JS, `tabExtent` is the only
 * function that changes.
 */

import type { TabId } from './types.js';

/**
 * Pointer travel (px) before a press becomes a drag. Below it a release is a
 * plain click; without it a pixel of drift between press and release would
 * reorder the strip by accident.
 */
export const REORDER_THRESHOLD = 4;

/** The tokens `tabExtent` reads. */
export interface ExtentTokens {
  strip: { rowHeight: number; tabMaxWidth: number; gap: number };
  space: { xs: number };
}

/** One tab's pitch along the strip's axis: its box plus the spacing after it. */
export function tabExtent(vertical: boolean, tokens: ExtentTokens): number {
  return vertical
    ? tokens.strip.rowHeight + tokens.space.xs
    : tokens.strip.tabMaxWidth + tokens.strip.gap;
}

/**
 * Where a tab pressed at `from` lands after the pointer moved `delta` px along
 * the axis, in a strip of `count` tabs. Clamped to the strip.
 */
export function dropIndex(from: number, delta: number, extent: number, count: number): number {
  if (count <= 0) return 0;
  const last = count - 1;
  if (!Number.isFinite(delta) || !(extent > 0)) return clamp(from, 0, last);
  return clamp(from + Math.round(delta / extent), 0, last);
}

/**
 * `items` with the element at `from` moved to `to`. Returns the *original*
 * array for a no-op (same index, or out of range) so `useSyncExternalStore`
 * consumers do not re-render on every pointer move that changes nothing.
 */
export function moveItem<T>(items: readonly T[], from: number, to: number): readonly T[] {
  if (from === to) return items;
  if (from < 0 || from >= items.length || to < 0 || to >= items.length) return items;
  const next = items.slice();
  const [moved] = next.splice(from, 1);
  next.splice(to, 0, moved as T);
  return next;
}

/** The local preview of a drag in progress. */
export interface TabDragPreview {
  tabId: TabId;
  /** The slot the tab is currently held over. */
  to: number;
}

/**
 * The strip's order while `drag` is in progress: the dragged tab moved to
 * `drag.to`. The tab is re-found by id rather than trusting the index it was
 * pressed at — a tab can close, or the server can reorder, mid-drag. Returns
 * the original array when nothing moves.
 */
export function previewOrder(tabIds: readonly TabId[], drag: TabDragPreview | null): readonly TabId[] {
  if (!drag) return tabIds;
  const from = tabIds.indexOf(drag.tabId);
  if (from === -1) return tabIds;
  return moveItem(tabIds, from, clamp(drag.to, 0, tabIds.length - 1));
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
