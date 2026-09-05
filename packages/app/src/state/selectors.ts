/**
 * Selectors over `WorkspaceState`. Plain functions; memoised per state
 * reference so a component that subscribes with one gets a stable value while
 * the state object does not change (05 §3).
 */

import { focusedSurfaceOf } from './layout.js';
import type { SessionView, SurfaceId, SurfaceView, TabId, TabView, WorkspaceState } from './types.js';

function memoPerState<T>(fn: (state: WorkspaceState) => T): (state: WorkspaceState) => T {
  const cache = new WeakMap<WorkspaceState, T>();
  return (state) => {
    const hit = cache.get(state);
    if (hit !== undefined) return hit;
    const value = fn(state);
    cache.set(state, value);
    return value;
  };
}

export const selectActiveSession = (state: WorkspaceState): SessionView | null =>
  state.activeSessionId === null ? null : (state.sessions[state.activeSessionId] ?? null);

export const selectSessions = memoPerState((state) =>
  state.sessionOrder.map((id) => state.sessions[id]).filter((s): s is SessionView => Boolean(s)),
);

export const selectActiveTabs = memoPerState((state): TabView[] => {
  const session = selectActiveSession(state);
  if (!session) return [];
  return session.tabIds.map((id) => state.tabs[id]).filter((t): t is TabView => Boolean(t));
});

export const selectActiveTabId = (state: WorkspaceState): TabId | null => {
  if (state.activeSessionId === null) return null;
  return state.activeTabBySession[state.activeSessionId] ?? null;
};

export const selectActiveTab = (state: WorkspaceState): TabView | null => {
  const id = selectActiveTabId(state);
  return id === null ? null : (state.tabs[id] ?? null);
};

/**
 * The focused Pane's Surface of a Tab: what the user chose (`pane.focus`) if
 * it is still one of the Tab's Panes, else the first Pane.
 */
export const selectFocusedSurfaceId = (state: WorkspaceState, tabId: TabId): SurfaceId | null => {
  const tab = state.tabs[tabId];
  return tab ? focusedSurfaceOf(tab, state.ui.focusedPaneByTab[tabId]) : null;
};

/** The active Tab's focused Pane's Surface — what Commands act on. */
export const selectActiveSurface = (state: WorkspaceState): SurfaceView | null => {
  const tab = selectActiveTab(state);
  if (!tab) return null;
  const id = selectFocusedSurfaceId(state, tab.id);
  return id === null ? null : (state.surfaces[id] ?? null);
};

/** The Tab's focused Pane's Surface (its title is what the tab row shows). */
export const selectSurfaceForTab = (state: WorkspaceState, tabId: TabId): SurfaceView | null => {
  const id = selectFocusedSurfaceId(state, tabId);
  return id === null ? null : (state.surfaces[id] ?? null);
};

/** Every Pane's Surface of a Tab that is known, in tree order. */
export function selectTabSurfaces(state: WorkspaceState, tabId: TabId): SurfaceView[] {
  const tab = state.tabs[tabId];
  if (!tab) return [];
  return tab.surfaceIds.map((id) => state.surfaces[id]).filter((s): s is SurfaceView => Boolean(s));
}

/**
 * Q44 as amended by ADR 0009: every Pane of the visible Tab mounts a
 * `<terminal-grid>`; nothing from other Tabs does. Warm Replicas for those
 * live in Rust.
 */
export const selectMountedSurfaceIds = memoPerState((state): number[] => {
  const tab = selectActiveTab(state);
  return tab ? [...tab.surfaceIds] : [];
});

/** The tab index of `tabId` inside the active session, or -1. */
export function selectTabIndex(state: WorkspaceState, tabId: TabId): number {
  const session = selectActiveSession(state);
  if (!session) return -1;
  return session.tabIds.indexOf(tabId);
}

/** Tab N (0-based) of the active session, for `tab.goto`. */
export function selectTabAt(state: WorkspaceState, index: number): TabView | null {
  const tabs = selectActiveTabs(state);
  return tabs[index] ?? null;
}

/** Neighbour tab in the active session, wrapping. */
export function selectRelativeTab(state: WorkspaceState, delta: number): TabView | null {
  const tabs = selectActiveTabs(state);
  if (tabs.length === 0) return null;
  const activeId = selectActiveTabId(state);
  const current = tabs.findIndex((t) => t.id === activeId);
  const base = current < 0 ? 0 : current;
  const next = (((base + delta) % tabs.length) + tabs.length) % tabs.length;
  return tabs[next] ?? null;
}

export const selectIsConnected = (state: WorkspaceState): boolean =>
  state.connection.status === 'connected';
