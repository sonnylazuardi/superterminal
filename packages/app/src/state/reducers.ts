/**
 * Pure reducers for the workspace projection (05 §3).
 *
 * No React, no I/O, no clock: everything here is a function of (state, input),
 * which is what makes the store testable in isolation. The server is
 * authoritative for sessions/tabs/surfaces/active* (Q17) — `applyServerEvent`
 * never invents them, and `applyUiAction` never touches them.
 */

import {
  layoutLeaves,
  tabLayout,
  type SurfaceMeta,
  type Workspace,
  type WorkspaceSnapshot,
} from '@superterminal/protocol-ts';
import { clampRatio, clampSidebarWidth } from './layout.js';
import type {
  ConnectionState,
  ServerEvent,
  SessionId,
  SessionView,
  SurfaceId,
  SurfaceView,
  TabId,
  TabView,
  UiAction,
  UiState,
  WorkspaceState,
} from './types.js';

export const initialUiState: UiState = {
  paletteOpen: false,
  paletteMode: 'commands',
  paletteQuery: '',
  paletteIndex: 0,
  verticalTabs: false,
  sidebarWidth: 220,
  focusedPaneByTab: {},
  menu: null,
  ratioPreview: null,
  renamingSessionId: null,
  confirmingCloseTabId: null,
  window: { width: 0, height: 0 },
  toasts: [],
  nextToastId: 1,
};

export const initialWorkspaceState: WorkspaceState = {
  connection: { status: 'connecting' },
  revision: 0,
  sessions: {},
  sessionOrder: [],
  activeSessionId: null,
  tabs: {},
  activeTabBySession: {},
  surfaces: {},
  ui: initialUiState,
};

/** Fold a `workspace.get`/`workspace.subscribe` result into the event stream. */
export function snapshotEvent(snapshot: WorkspaceSnapshot): ServerEvent {
  return { t: 'snapshot', snapshot };
}

/* ------------------------------------------------------------ projection -- */

function surfaceFromMeta(meta: SurfaceMeta, previous?: SurfaceView): SurfaceView {
  const exited = meta.state.kind === 'exited' ? meta.state : null;
  return {
    id: meta.id,
    title: meta.user_title ?? meta.title,
    cwd: meta.cwd ?? previous?.cwd ?? '',
    status: exited ? 'exited' : 'running',
    exitCode: exited ? exited.code : null,
    exitSignal: exited ? exited.signal : null,
    hasForegroundChild: meta.has_foreground_child,
    // The bell is a data-plane event; the document never clears it.
    bell: previous?.bell ?? false,
    cols: meta.cols,
    rows: meta.rows,
  };
}

function projectWorkspace(
  state: WorkspaceState,
  revision: number,
  workspace: Workspace,
  surfaces: SurfaceMeta[],
): WorkspaceState {
  const sessions: Record<SessionId, SessionView> = {};
  const sessionOrder: SessionId[] = [];
  const tabs: Record<TabId, TabView> = {};
  const activeTabBySession: Record<SessionId, TabId> = {};

  for (const session of workspace.sessions) {
    const tabIds: TabId[] = [];
    for (const tab of session.tabs) {
      const layout = tabLayout(tab);
      tabs[tab.id] = {
        id: tab.id,
        sessionId: session.id,
        surfaceId: tab.surface,
        layout,
        surfaceIds: layoutLeaves(layout),
      };
      tabIds.push(tab.id);
    }
    sessions[session.id] = { id: session.id, name: session.name, tabIds };
    sessionOrder.push(session.id);
    if (session.active_tab !== null && tabs[session.active_tab]) {
      activeTabBySession[session.id] = session.active_tab;
    } else if (tabIds.length > 0) {
      // The server always sends one, but never render a session with no active
      // tab: fall back to the first.
      activeTabBySession[session.id] = tabIds[0]!;
    }
  }

  const nextSurfaces: Record<number, SurfaceView> = {};
  for (const meta of surfaces) {
    nextSurfaces[meta.id] = surfaceFromMeta(meta, state.surfaces[meta.id]);
  }

  const activeSessionId = sessions[workspace.active_session]
    ? workspace.active_session
    : (sessionOrder[0] ?? null);

  const ui = pruneUi(state.ui, sessions, tabs);

  return {
    ...state,
    revision,
    sessions,
    sessionOrder,
    activeSessionId,
    tabs,
    activeTabBySession,
    surfaces: nextSurfaces,
    ui,
  };
}

/** Drop UI state that points at things the server no longer has. */
function pruneUi(
  ui: UiState,
  sessions: Record<SessionId, SessionView>,
  tabs: Record<TabId, TabView>,
): UiState {
  const renamingGone = ui.renamingSessionId !== null && !sessions[ui.renamingSessionId];
  const confirmGone = ui.confirmingCloseTabId !== null && !tabs[ui.confirmingCloseTabId];
  const menuGone = ui.menu !== null && !tabs[ui.menu.tabId];
  const previewGone = ui.ratioPreview !== null && !tabs[ui.ratioPreview.tabId];
  // A focused Pane that closed (or a Tab that closed) drops out; the Tab then
  // focuses its first Pane through the selector's fallback.
  let focused = ui.focusedPaneByTab;
  for (const [key, surfaceId] of Object.entries(ui.focusedPaneByTab)) {
    const tab = tabs[Number(key)];
    if (tab && tab.surfaceIds.includes(surfaceId)) continue;
    if (focused === ui.focusedPaneByTab) focused = { ...focused };
    delete focused[Number(key)];
  }
  if (!renamingGone && !confirmGone && !menuGone && !previewGone && focused === ui.focusedPaneByTab) {
    return ui;
  }
  return {
    ...ui,
    renamingSessionId: renamingGone ? null : ui.renamingSessionId,
    confirmingCloseTabId: confirmGone ? null : ui.confirmingCloseTabId,
    menu: menuGone ? null : ui.menu,
    ratioPreview: previewGone ? null : ui.ratioPreview,
    focusedPaneByTab: focused,
  };
}

/* ------------------------------------------------------- server reducer -- */

export function applyServerEvent(state: WorkspaceState, event: ServerEvent): WorkspaceState {
  switch (event.t) {
    case 'snapshot':
      return projectWorkspace(
        state,
        event.snapshot.workspace.revision,
        event.snapshot.workspace,
        event.snapshot.surfaces,
      );

    case 'ev.workspace':
      // Stale pushes can overtake a newer snapshot after a reconnect.
      if (event.revision < state.revision) return state;
      return projectWorkspace(state, event.revision, event.workspace, event.surfaces);

    case 'ev.surface_exited': {
      const surface = state.surfaces[event.surface];
      if (!surface) return state;
      if (
        surface.status === 'exited' &&
        surface.exitCode === event.code &&
        surface.exitSignal === event.signal
      ) {
        return state;
      }
      return {
        ...state,
        surfaces: {
          ...state.surfaces,
          [event.surface]: {
            ...surface,
            status: 'exited',
            exitCode: event.code,
            exitSignal: event.signal,
            hasForegroundChild: false,
          },
        },
      };
    }

    case 'ev.server_shutting_down':
      if (
        state.connection.status === 'failed' &&
        state.connection.error === `server shutting down: ${event.reason}`
      ) {
        return state;
      }
      return {
        ...state,
        connection: {
          ...state.connection,
          status: 'failed',
          error: `server shutting down: ${event.reason}`,
        },
      };

    default:
      // Unknown `ev.*` is minor-version compatible (02 §10): ignore it.
      return state;
  }
}

/* ----------------------------------------------------------- ui reducer -- */

export function applyUiAction(state: WorkspaceState, action: UiAction): WorkspaceState {
  const ui = state.ui;
  switch (action.type) {
    case 'connection.set': {
      const serverVersion = action.serverVersion ?? state.connection.serverVersion;
      const serverBuildId = action.serverBuildId ?? state.connection.serverBuildId;
      const next: ConnectionState = {
        status: action.status,
        ...(serverVersion !== undefined ? { serverVersion } : {}),
        ...(serverBuildId !== undefined ? { serverBuildId } : {}),
        ...(action.error !== undefined ? { error: action.error } : {}),
      };
      if (
        next.status === state.connection.status &&
        next.serverVersion === state.connection.serverVersion &&
        next.serverBuildId === state.connection.serverBuildId &&
        next.error === state.connection.error
      ) {
        return state;
      }
      return { ...state, connection: next };
    }

    case 'palette.open':
      return withUi(state, {
        paletteOpen: true,
        paletteMode: action.mode ?? ui.paletteMode,
        paletteQuery: '',
        paletteIndex: 0,
      });

    case 'palette.close':
      if (!ui.paletteOpen) return state;
      return withUi(state, { paletteOpen: false, paletteQuery: '', paletteIndex: 0 });

    case 'palette.setMode':
      if (ui.paletteMode === action.mode) return state;
      return withUi(state, { paletteMode: action.mode, paletteQuery: '', paletteIndex: 0 });

    case 'palette.setQuery':
      if (ui.paletteQuery === action.query) return state;
      return withUi(state, { paletteQuery: action.query, paletteIndex: 0 });

    case 'palette.move': {
      if (action.count <= 0) return ui.paletteIndex === 0 ? state : withUi(state, { paletteIndex: 0 });
      const wrapped = (((ui.paletteIndex + action.delta) % action.count) + action.count) % action.count;
      if (wrapped === ui.paletteIndex) return state;
      return withUi(state, { paletteIndex: wrapped });
    }

    case 'palette.setIndex': {
      if (action.index === ui.paletteIndex) return state;
      return withUi(state, { paletteIndex: Math.max(0, action.index) });
    }

    case 'ui.toggleVerticalTabs':
      return withUi(state, { verticalTabs: !ui.verticalTabs });

    case 'ui.setVerticalTabs':
      if (ui.verticalTabs === action.value) return state;
      return withUi(state, { verticalTabs: action.value });

    case 'ui.setSidebarWidth': {
      const width = clampSidebarWidth(action.width);
      if (ui.sidebarWidth === width) return state;
      return withUi(state, { sidebarWidth: width });
    }

    case 'pane.focus': {
      // Membership is not checked: a `tab.split` result can land before the
      // snapshot that adds the Pane. Selectors fall back to the first Pane
      // until it does, and `pruneUi` drops it if it never arrives.
      if (!state.tabs[action.tabId]) return state;
      if (ui.focusedPaneByTab[action.tabId] === action.surfaceId) return state;
      return withUi(state, {
        focusedPaneByTab: { ...ui.focusedPaneByTab, [action.tabId]: action.surfaceId },
      });
    }

    case 'menu.open':
      if (!state.tabs[action.tabId]) return state;
      return withUi(state, { menu: { tabId: action.tabId, x: action.x, y: action.y, index: 0 } });

    case 'menu.close':
      if (ui.menu === null) return state;
      return withUi(state, { menu: null });

    case 'menu.move': {
      if (ui.menu === null || action.count <= 0) return state;
      const next = (((ui.menu.index + action.delta) % action.count) + action.count) % action.count;
      if (next === ui.menu.index) return state;
      return withUi(state, { menu: { ...ui.menu, index: next } });
    }

    case 'ratio.preview': {
      if (!state.tabs[action.tabId]) return state;
      const ratio = clampRatio(action.ratio);
      const current = ui.ratioPreview;
      if (
        current &&
        current.tabId === action.tabId &&
        current.ratio === ratio &&
        current.path.length === action.path.length &&
        current.path.every((step, i) => step === action.path[i])
      ) {
        return state;
      }
      return withUi(state, { ratioPreview: { tabId: action.tabId, path: [...action.path], ratio } });
    }

    case 'ratio.clear':
      if (ui.ratioPreview === null) return state;
      return withUi(state, { ratioPreview: null });

    case 'session.beginRename':
      if (!state.sessions[action.sessionId]) return state;
      if (ui.renamingSessionId === action.sessionId) return state;
      return withUi(state, { renamingSessionId: action.sessionId });

    case 'session.endRename':
      if (ui.renamingSessionId === null) return state;
      return withUi(state, { renamingSessionId: null });

    case 'tab.confirmClose':
      if (ui.confirmingCloseTabId === action.tabId) return state;
      if (action.tabId !== null && !state.tabs[action.tabId]) return state;
      return withUi(state, { confirmingCloseTabId: action.tabId });

    case 'surface.bell': {
      const surface = state.surfaces[action.surfaceId];
      if (!surface || surface.bell) return state;
      return {
        ...state,
        surfaces: { ...state.surfaces, [action.surfaceId]: { ...surface, bell: true } },
      };
    }

    case 'surface.clearBell': {
      const surface = state.surfaces[action.surfaceId];
      if (!surface || !surface.bell) return state;
      return {
        ...state,
        surfaces: { ...state.surfaces, [action.surfaceId]: { ...surface, bell: false } },
      };
    }

    case 'window.resize':
      if (ui.window.width === action.width && ui.window.height === action.height) return state;
      return withUi(state, { window: { width: action.width, height: action.height } });

    case 'toast.push':
      return withUi(state, {
        toasts: [
          ...ui.toasts,
          { id: ui.nextToastId, text: action.text, kind: action.kind ?? 'info' },
        ],
        nextToastId: ui.nextToastId + 1,
      });

    case 'toast.dismiss': {
      const toasts = ui.toasts.filter((t) => t.id !== action.id);
      if (toasts.length === ui.toasts.length) return state;
      return withUi(state, { toasts });
    }

    default: {
      // Exhaustiveness: a new action must be handled above.
      const never: never = action;
      return never;
    }
  }
}

function withUi(state: WorkspaceState, patch: Partial<UiState>): WorkspaceState {
  return { ...state, ui: { ...state.ui, ...patch } };
}
