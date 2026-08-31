import type {
  Ev,
  SessionId,
  SurfaceId,
  TabId,
  WorkspaceSnapshot,
} from '@superterminal/protocol-ts';

export type { SessionId, SurfaceId, TabId };

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
  surfaceId: SurfaceId;
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

export interface UiState {
  paletteOpen: boolean;
  paletteMode: PaletteMode;
  paletteQuery: string;
  paletteIndex: number;
  verticalTabs: boolean;
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
  | { type: 'session.beginRename'; sessionId: SessionId }
  | { type: 'session.endRename' }
  | { type: 'tab.confirmClose'; tabId: TabId | null }
  | { type: 'surface.bell'; surfaceId: SurfaceId }
  | { type: 'surface.clearBell'; surfaceId: SurfaceId }
  | { type: 'window.resize'; width: number; height: number }
  | { type: 'toast.push'; text: string; kind?: Toast['kind'] }
  | { type: 'toast.dismiss'; id: number };
