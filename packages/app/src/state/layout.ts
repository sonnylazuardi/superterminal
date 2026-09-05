/**
 * Pure helpers over a Tab's Pane tree (ADR 0009) and the drag geometry the
 * chrome needs. No React, no store, no I/O — everything here is unit-tested
 * without a window.
 *
 * # Drag geometry without element bounds
 *
 * gpuix reports pointer positions in window coordinates but exposes no
 * element bounds to JavaScript, so a divider cannot ask "how big is the Split
 * I belong to". The extent is computed instead: the content area is the
 * window minus the chrome (title bar, sidebar or strip), and every nested
 * Split hands its children `ratio` / `1 - ratio` of its own extent along its
 * axis, minus the divider. That is exactly what flexbox lays out, so the
 * numbers agree with the paint as long as the chrome tokens do.
 *
 * The drag itself is delta-based — `startRatio + (pointer - start) / extent`
 * — so only the extent matters, never the origin. A slightly wrong origin (the
 * connection banner, say) cannot make the divider jump under the pointer.
 */

import type { Layout, SplitAxis, SplitPath, SurfaceId } from '@superterminal/protocol-ts';

/** The Tab's focused Pane: the chosen one while it still exists, else the first. */
export function focusedSurfaceOf(
  tab: { surfaceId: SurfaceId; surfaceIds: SurfaceId[] },
  chosen: SurfaceId | undefined,
): SurfaceId {
  if (chosen !== undefined && tab.surfaceIds.includes(chosen)) return chosen;
  return tab.surfaceIds[0] ?? tab.surfaceId;
}

/** Total thickness of a Divider's grab band, in px. Odd so the hairline centres. */
export const DIVIDER_GRAB = 7;

/** The coordinate a drag along `axis` cares about, from a gpuix mouse payload. */
export function dragPosition(
  axis: SplitAxis,
  event: { x?: number; y?: number },
): number | null {
  const pos = axis === 'row' ? event.x : event.y;
  return typeof pos === 'number' && Number.isFinite(pos) ? pos : null;
}

/**
 * The rectangle the Pane tree is laid out in, in window coordinates, derived
 * from the chrome tokens rather than measured. Must agree with what
 * `App.tsx` stacks above and beside the content: the title bar (both modes),
 * the sidebar column plus its Divider band (vertical) or the tab strip
 * (horizontal).
 */
export function contentRect(
  window: { width: number; height: number },
  verticalTabs: boolean,
  sidebarWidth: number,
  tokens: { strip: { titleBarHeight: number; height: number } },
): Rect {
  const left = verticalTabs ? sidebarWidth + DIVIDER_GRAB : 0;
  const top = verticalTabs
    ? tokens.strip.titleBarHeight
    : tokens.strip.titleBarHeight + tokens.strip.height;
  return {
    x: left,
    y: top,
    width: Math.max(0, window.width - left),
    height: Math.max(0, window.height - top),
  };
}

export interface MenuItem {
  commandId: string;
  title: string;
}

/** Which Commands the tab Menu offers for a Tab, in order. */
export function menuItemsFor(tab: { surfaceIds: SurfaceId[] } | undefined): MenuItem[] {
  if (!tab) return [];
  const items: MenuItem[] = [
    { commandId: 'pane.splitRight', title: 'Split Right' },
    { commandId: 'pane.splitDown', title: 'Split Down' },
  ];
  if (tab.surfaceIds.length > 1) items.push({ commandId: 'pane.close', title: 'Close Pane' });
  items.push({ commandId: 'tab.close', title: 'Close Tab' });
  return items;
}

/** Sidebar width bounds, in logical px. The default is `tokens.strip.verticalWidth`. */
export const SIDEBAR_WIDTH_MIN = 160;
export const SIDEBAR_WIDTH_MAX = 480;

export function clampSidebarWidth(width: number): number {
  if (!Number.isFinite(width)) return SIDEBAR_WIDTH_MIN;
  return Math.min(SIDEBAR_WIDTH_MAX, Math.max(SIDEBAR_WIDTH_MIN, Math.round(width)));
}

/** A Split never gets thinner than a tenth of its parent on either side. */
export const RATIO_MIN = 0.1;
export const RATIO_MAX = 0.9;

export function clampRatio(ratio: number): number {
  if (!Number.isFinite(ratio)) return 0.5;
  return Math.min(RATIO_MAX, Math.max(RATIO_MIN, ratio));
}

/** The Split node at `path`, or null when the path leaves the tree. */
export function splitAt(layout: Layout, path: SplitPath): Extract<Layout, { kind: 'split' }> | null {
  let node: Layout = layout;
  for (const step of path) {
    if (node.kind !== 'split') return null;
    node = step === 0 ? node.first : node.second;
  }
  return node.kind === 'split' ? node : null;
}

/** The first Pane inside `layout` (tree order). */
export function firstLeaf(layout: Layout): SurfaceId {
  return layout.kind === 'leaf' ? layout.surface : firstLeaf(layout.first);
}

/**
 * The Pane that keeps focus when `surface` closes: its sibling subtree's first
 * leaf, or null when `surface` is the only Pane (the Tab itself closes).
 */
export function siblingLeaf(layout: Layout, surface: SurfaceId): SurfaceId | null {
  if (layout.kind === 'leaf') return null;
  if (layout.first.kind === 'leaf' && layout.first.surface === surface) {
    return firstLeaf(layout.second);
  }
  if (layout.second.kind === 'leaf' && layout.second.surface === surface) {
    return firstLeaf(layout.first);
  }
  return siblingLeaf(layout.first, surface) ?? siblingLeaf(layout.second, surface);
}

/** Neighbour Pane in tree order, wrapping. */
export function relativeLeaf(leaves: SurfaceId[], current: SurfaceId, delta: number): SurfaceId | null {
  if (leaves.length === 0) return null;
  const index = leaves.indexOf(current);
  const base = index < 0 ? 0 : index;
  const next = (((base + delta) % leaves.length) + leaves.length) % leaves.length;
  return leaves[next] ?? null;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Where a Split's divider lives and how far it can travel. */
export interface SplitGeometry {
  axis: SplitAxis;
  /** Size of the Split along its axis, divider included. */
  extent: number;
  rect: Rect;
}

/**
 * Geometry of every Split in `layout` laid out inside `rect`, keyed by the
 * Split's path joined with `/` (`''` is the root). `dividerSize` is the total
 * thickness the divider takes out of the flow. `preview` overrides one
 * Split's ratio while it is being dragged so nested Splits follow it.
 */
export function splitGeometry(
  layout: Layout,
  rect: Rect,
  dividerSize: number,
  preview: { path: SplitPath; ratio: number } | null = null,
): Map<string, SplitGeometry> {
  const out = new Map<string, SplitGeometry>();
  walk(layout, rect, []);
  return out;

  function walk(node: Layout, box: Rect, path: SplitPath): void {
    if (node.kind !== 'split') return;
    const key = pathKey(path);
    const ratio =
      preview && pathKey(preview.path) === key ? clampRatio(preview.ratio) : clampRatio(node.ratio);
    const extent = node.axis === 'row' ? box.width : box.height;
    out.set(key, { axis: node.axis, extent, rect: box });
    const free = Math.max(0, extent - dividerSize);
    const firstSize = free * ratio;
    const secondSize = free - firstSize;
    if (node.axis === 'row') {
      walk(node.first, { ...box, width: firstSize }, [...path, 0]);
      walk(node.second, { ...box, x: box.x + firstSize + dividerSize, width: secondSize }, [
        ...path,
        1,
      ]);
    } else {
      walk(node.first, { ...box, height: firstSize }, [...path, 0]);
      walk(node.second, { ...box, y: box.y + firstSize + dividerSize, height: secondSize }, [
        ...path,
        1,
      ]);
    }
  }
}

export function pathKey(path: SplitPath): string {
  return path.join('/');
}

/**
 * The ratio a divider drag produces: the ratio at mouse-down plus the pointer
 * travel as a fraction of the Split's extent, clamped. A zero extent (the
 * window has not been measured yet) leaves the ratio alone.
 */
export function dragRatio(startRatio: number, startPos: number, pos: number, extent: number): number {
  if (!(extent > 0)) return clampRatio(startRatio);
  return clampRatio(startRatio + (pos - startPos) / extent);
}
