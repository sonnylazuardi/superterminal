import type {
  Ev,
  Layout,
  SessionId,
  SplitPath,
  SurfaceId,
  TabId,
  WorkspaceSnapshot,
} from '@superterminal/protocol-ts';

export type { Layout, SessionId, SplitPath, SurfaceId, TabId };

export type ConnectionStatus =
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'failed'
  | 'mismatch'
  | 'closed';

export interface ConnectionState {
  status: ConnectionStatus;
  serverVersion?: string;
  serverBuildId?: string;
  error?: string;
}

export interface SessionView {
  id: SessionId;
  name: string;
  tabIds: TabId[];
}

export interface TabView {
  id: TabId;
  sessionId: SessionId;
  /** The first Pane's Surface (`Tab.surface` on the wire). */
  surfaceId: SurfaceId;
  /** The Pane tree (ADR 0009); a 1.0 daemon implies a single leaf. */
  layout: Layout;
  /** Every Pane's Surface in tree order — `layoutLeaves(layout)`. */
  surfaceIds: SurfaceId[];
}

export type SurfaceStatus = 'starting' | 'running' | 'exited';

export interface SurfaceView {
  id: SurfaceId;
  /** `user_title` when set, else the OSC title (02 §3.2). */
  title: string;
  cwd: string;
  status: SurfaceStatus;
  exitCode: number | null;
  exitSignal: string | null;
  /** Q48: drives the close-tab confirmation. */
  hasForegroundChild: boolean;
  /** Data-plane bell, cleared when the tab is looked at. */
  bell: boolean;
  cols: number;
  rows: number;
}

export type PaletteMode = 'commands' | 'sessions';

export interface Toast {
  id: number;
  text: string;
  kind: 'info' | 'error';
}

/** A right-click Menu on a tab row, opened at the pointer (window px). */
export interface MenuState {
  tabId: TabId;
  x: number;
  y: number;
  index: number;
}

/** Local live preview of a Split's ratio while its divider is dragged. */
export interface RatioPreview {
  tabId: TabId;
  path: SplitPath;
  ratio: number;
}

export interface UiState {
  paletteOpen: boolean;
  paletteMode: PaletteMode;
  paletteQuery: string;
  paletteIndex: number;
  verticalTabs: boolean;
  /** Sidebar column width in logical px (Client State). */
  sidebarWidth: number;
  /** The focused Pane per Tab; a Tab absent here focuses its first Pane. */
  focusedPaneByTab: Record<TabId, SurfaceId>;
  menu: MenuState | null;
  ratioPreview: RatioPreview | null;
  renamingSessionId: SessionId | null;
  /** Tab awaiting a "really close? something is running" confirmation. */
  confirmingCloseTabId: TabId | null;
  window: { width: number; height: number };
  toasts: Toast[];
  nextToastId: number;
}

export interface WorkspaceState {
  connection: ConnectionState;
  /** Workspace document revision the projection was built from. */
  revision: number;
  sessions: Record<SessionId, SessionView>;
  sessionOrder: SessionId[];
  activeSessionId: SessionId | null;
  tabs: Record<TabId, TabView>;
  activeTabBySession: Record<SessionId, TabId>;
  surfaces: Record<SurfaceId, SurfaceView>;
  ui: UiState;
}

/**
 * What the reducer consumes. `workspace.subscribe`/`workspace.get` replies are
 * folded in as a synthetic `snapshot` event so there is one code path.
 */
export type ServerEvent = Ev | { t: 'snapshot'; snapshot: WorkspaceSnapshot };

export type UiAction =
  | { type: 'connection.set'; status: ConnectionStatus; serverVersion?: string; serverBuildId?: string; error?: string }
  | { type: 'palette.open'; mode?: PaletteMode }
  | { type: 'palette.close' }
  | { type: 'palette.setMode'; mode: PaletteMode }
  | { type: 'palette.setQuery'; query: string }
  | { type: 'palette.move'; delta: number; count: number }
  | { type: 'palette.setIndex'; index: number }
  | { type: 'ui.toggleVerticalTabs' }
  | { type: 'ui.setVerticalTabs'; value: boolean }
  | { type: 'ui.setSidebarWidth'; width: number }
  | { type: 'pane.focus'; tabId: TabId; surfaceId: SurfaceId }
  | { type: 'menu.open'; tabId: TabId; x: number; y: number }
  | { type: 'menu.close' }
  | { type: 'menu.move'; delta: number; count: number }
  | { type: 'ratio.preview'; tabId: TabId; path: SplitPath; ratio: number }
  | { type: 'ratio.clear' }
  | { type: 'session.beginRename'; sessionId: SessionId }
  | { type: 'session.endRename' }
  | { type: 'tab.confirmClose'; tabId: TabId | null }
  | { type: 'surface.bell'; surfaceId: SurfaceId }
  | { type: 'surface.clearBell'; surfaceId: SurfaceId }
  | { type: 'window.resize'; width: number; height: number }
  | { type: 'toast.push'; text: string; kind?: Toast['kind'] }
  | { type: 'toast.dismiss'; id: number };
