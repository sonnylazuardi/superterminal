// Hand-written mirror of crates/st-proto; task M4-05 replaces this with ts-rs
// generation, CI diff-checks it.
//
// Source of truth: docs/plan/02-protocol.md §2 (handshake), §3 (CONTROL
// messages), amended by docs/plan/00-grilling.md §F:
//   - Q43: selection / scroll offset edits travel on the DATA plane. `view.set`
//     stays here for tooling and tests only; the app never sends it.
//   - Q48: `tab.set_active` exists; the surface record carries `cwd` and
//     `has_foreground_child`.
//
// Only the CONTROL plane (newline-delimited JSON) is modelled here. The DATA
// plane is postcard-framed binary owned by the Rust client core (04).

/* ------------------------------------------------------------------ ids -- */

export type SessionId = number;
export type TabId = number;
export type SurfaceId = number;

/* ------------------------------------------------------------ handshake -- */

/** Structured form. On the CONTROL plane the wire carries `"major.minor"`. */
export interface ProtoVersion {
  major: number;
  minor: number;
}

/** 1.1 added `Tab.layout`, `tab.split`, `pane.close`, `tab.set_ratio` (ADR 0009). */
export const PROTO_VERSION: ProtoVersion = { major: 1, minor: 1 };

export function formatProtoVersion(v: ProtoVersion): string {
  return `${v.major}.${v.minor}`;
}

export function parseProtoVersion(s: string): ProtoVersion | null {
  const m = /^(\d+)\.(\d+)$/.exec(s.trim());
  if (!m) return null;
  return { major: Number(m[1]), minor: Number(m[2]) };
}

/** `Data` never appears on this plane; it is listed for completeness (02 §2). */
export type ClientKind = 'control' | 'data' | 'tool';

export type RejectReason =
  | 'major_mismatch'
  | 'bad_magic'
  | 'line_too_long'
  | 'frame_too_large'
  | 'not_hello'
  | 'shutting_down';

export interface Hello {
  t: 'hello';
  proto_version: string;
  client_kind: 'control' | 'tool';
  build_id: string;
}

export interface HelloAck {
  t: 'hello.ack';
  proto_version: string;
  server_build_id: string;
  workspace_revision: number;
  server_pid: number;
}

export interface Reject {
  t: 'reject';
  reason: RejectReason | string;
  message: string;
  server_version: string;
}

/* ---------------------------------------------------------------- error -- */

export type ErrorCode =
  | 'bad_request' // malformed / unknown t / missing field
  | 'not_found' // unknown session/tab/surface id
  | 'conflict' // if_revision did not match current workspace revision
  | 'spawn_failed' // PTY/shell could not start; message has errno text
  | 'unsupported' // message exists but not in the negotiated minor
  | 'shutting_down'
  | 'internal';

export const ERROR_CODES: readonly ErrorCode[] = [
  'bad_request',
  'not_found',
  'conflict',
  'spawn_failed',
  'unsupported',
  'shutting_down',
  'internal',
] as const;

export function isErrorCode(x: unknown): x is ErrorCode {
  return typeof x === 'string' && (ERROR_CODES as readonly string[]).includes(x);
}

export interface ErrorBody {
  code: ErrorCode;
  message: string;
  data?: unknown;
}

export interface ErrRes {
  t: 'err';
  id: number;
  error: ErrorBody;
}

export interface OkRes<R> {
  t: 'ok';
  id: number;
  result: R;
}

/* ------------------------------------------------ workspace document Q17 -- */

export interface Workspace {
  /** Increments on every change. */
  revision: number;
  active_session: SessionId;
  /** Ordered. */
  sessions: Session[];
}

export interface Session {
  id: SessionId;
  name: string;
  active_tab: TabId | null;
  tabs: Tab[];
}

/** Flex direction of a Split: `row` = side by side (Split Right), `column` = stacked (Split Down). */
export type SplitAxis = 'row' | 'column';

/**
 * A Tab's layout tree (ADR 0009). Leaves are Panes, each showing one Surface.
 * A Split node is addressed by its path from the root: 0 = `first`, 1 = `second`.
 */
export type Layout =
  | { kind: 'leaf'; surface: SurfaceId }
  | { kind: 'split'; axis: SplitAxis; ratio: number; first: Layout; second: Layout };

/** Path of a Split node from the root: 0 = first child, 1 = second. `[]` is the root. */
export type SplitPath = number[];

export interface Tab {
  id: TabId;
  /** The first leaf of `layout`; kept so 1.0 readers keep working. */
  surface: SurfaceId;
  /** Absent from a 1.0 daemon: treat as `{ kind: 'leaf', surface }`. */
  layout?: Layout;
}

/** The layout a 1.0 daemon (no `layout` field) implies. */
export function tabLayout(tab: Tab): Layout {
  return tab.layout ?? { kind: 'leaf', surface: tab.surface };
}

/** Surfaces of every Pane in tree order (first before second). */
export function layoutLeaves(layout: Layout): SurfaceId[] {
  return layout.kind === 'leaf'
    ? [layout.surface]
    : [...layoutLeaves(layout.first), ...layoutLeaves(layout.second)];
}

export type SurfaceState =
  | { kind: 'running' }
  | { kind: 'exited'; code: number | null; signal: string | null };

export interface SurfaceMeta {
  id: SurfaceId;
  title: string;
  /** From `surface.rename`; overrides `title` in the UI when set. */
  user_title: string | null;
  cwd: string | null;
  cols: number;
  rows: number;
  state: SurfaceState;
  view_state: ViewState;
  /**
   * Q48: the server samples the PTY foreground process group; true when a
   * process other than the shell owns it. Drives the close-tab confirmation.
   */
  has_foreground_child: boolean;
}

export interface ViewState {
  /** Lines above the bottom; 0 = following output. */
  scroll_offset: number;
  selection: Selection | null;
}

export interface Selection {
  kind: 'normal' | 'block' | 'lines';
  /** `line` is an absolute line id (02 §8) so it survives scrolling. */
  anchor: { line: number; col: number };
  head: { line: number; col: number };
}

export interface WorkspaceSnapshot {
  workspace: Workspace;
  surfaces: SurfaceMeta[];
}

/* -------------------------------------------------------------- requests -- */

export interface SpawnSpec {
  /** argv; default from config.toml. */
  shell?: string[];
  /** Default: config / `$HOME`. */
  cwd?: string;
  /** Merged over the server's environment (Q48 allow-list). */
  env?: Record<string, string>;
  cols: number;
  rows: number;
}

export type Signal = 'HUP' | 'TERM' | 'KILL';

export type Req =
  // workspace
  | { t: 'workspace.get'; id: number }
  | { t: 'workspace.subscribe'; id: number }
  // sessions
  | { t: 'session.create'; id: number; name: string; if_revision?: number }
  | { t: 'session.rename'; id: number; session: SessionId; name: string; if_revision?: number }
  | { t: 'session.delete'; id: number; session: SessionId; if_revision?: number }
  | { t: 'session.list'; id: number }
  | { t: 'session.set_active'; id: number; session: SessionId }
  // tabs — exactly one of spawn|surface
  | {
      t: 'tab.create';
      id: number;
      session: SessionId;
      index?: number;
      spawn?: SpawnSpec;
      surface?: SurfaceId;
      if_revision?: number;
    }
  | { t: 'tab.close'; id: number; tab: TabId; if_revision?: number }
  | { t: 'tab.reorder'; id: number; tab: TabId; index: number; if_revision?: number }
  | {
      t: 'tab.move';
      id: number;
      tab: TabId;
      to_session: SessionId;
      index?: number;
      if_revision?: number;
    }
  | { t: 'tab.set_active'; id: number; tab: TabId }
  // panes (ADR 0009) — `pane` is the Surface shown in the Pane being split/closed
  | {
      t: 'tab.split';
      id: number;
      tab: TabId;
      pane: SurfaceId;
      axis: SplitAxis;
      spawn: SpawnSpec;
      if_revision?: number;
    }
  | { t: 'pane.close'; id: number; tab: TabId; pane: SurfaceId; if_revision?: number }
  | { t: 'tab.set_ratio'; id: number; tab: TabId; path: SplitPath; ratio: number; if_revision?: number }
  // surfaces
  | { t: 'surface.create'; id: number; spawn: SpawnSpec }
  | { t: 'surface.kill'; id: number; surface: SurfaceId; signal?: Signal }
  | { t: 'surface.rename'; id: number; surface: SurfaceId; user_title: string | null }
  // view state (Q17, Q24) — tooling/tests only; the app never sends this (Q43)
  | {
      t: 'view.set';
      id: number;
      surface: SurfaceId;
      scroll_offset?: number;
      selection?: Selection | null;
    }
  // server
  | { t: 'server.status'; id: number }
  | { t: 'server.shutdown'; id: number; force?: boolean };

export interface ServerStatus {
  build_id: string;
  proto_version: string;
  pid: number;
  uptime_s: number;
  surfaces: number;
  control_clients: number;
  data_clients: number;
  /** `$XDG_STATE_HOME/superterminal/workspace.json` (Q18). */
  workspace_file: string;
}

export interface ResultMap {
  'workspace.get': WorkspaceSnapshot;
  /** Initial state, then `ev.workspace` pushes. */
  'workspace.subscribe': WorkspaceSnapshot;
  'session.create': { session: SessionId; revision: number };
  'session.rename': { revision: number };
  'session.delete': { revision: number };
  'session.list': { sessions: Session[] };
  'session.set_active': { revision: number };
  'tab.create': { tab: TabId; surface: SurfaceId; revision: number };
  'tab.close': { revision: number };
  'tab.reorder': { revision: number };
  'tab.move': { revision: number };
  'tab.set_active': { revision: number };
  /** The new Pane's Surface. */
  'tab.split': { tab: TabId; surface: SurfaceId; revision: number };
  'pane.close': { revision: number };
  'tab.set_ratio': { revision: number };
  'surface.create': { surface: SurfaceId };
  'surface.kill': Record<string, never>;
  'surface.rename': { revision: number };
  'view.set': { revision: number };
  'server.status': ServerStatus;
  'server.shutdown': Record<string, never>;
}

/* ---------------------------------------------------------------- events -- */

export type Ev =
  /** Full document; it is a few KB even for dozens of tabs. */
  | { t: 'ev.workspace'; revision: number; workspace: Workspace; surfaces: SurfaceMeta[] }
  | { t: 'ev.surface_exited'; surface: SurfaceId; code: number | null; signal: string | null }
  | { t: 'ev.server_shutting_down'; reason: string };

export type EvType = Ev['t'];

export type Res = OkRes<ResultMap[keyof ResultMap]> | ErrRes;

export type ControlMsg = Req | Res | Ev | Hello | HelloAck | Reject;

/* --------------------------------------------------------- helper types -- */

/** Every `t` the client may send as a request. */
export type RequestType = Req['t'];

/** The fields of request `M` other than the envelope (`t`, `id`). */
export type ReqParams<M extends RequestType> = Omit<Extract<Req, { t: M }>, 't' | 'id'>;

/** The `result` payload of a successful response to request `M`. */
export type ResOk<M extends RequestType> = ResultMap[M];

/* --------------------------------------------------------- narrow guards -- */

export function isOkRes(m: ControlMsg): m is OkRes<ResultMap[keyof ResultMap]> {
  return m.t === 'ok';
}

export function isErrRes(m: ControlMsg): m is ErrRes {
  return m.t === 'err';
}

export function isEvent(m: ControlMsg): m is Ev {
  return typeof m.t === 'string' && m.t.startsWith('ev.');
}
