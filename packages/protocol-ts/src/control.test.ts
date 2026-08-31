// Type-level conformance tests for the hand-written control-plane mirror.
//
// Every member of every wire union is constructed here with an explicit type
// annotation, so `tsc --noEmit` fails if a variant is renamed, retyped or
// dropped; the runtime assertions keep `bun test` honest about the tags.

import { expect, test } from 'bun:test';
import {
  ERROR_CODES,
  PROTO_VERSION,
  PROTO_VERSION_STRING,
  formatProtoVersion,
  isErrRes,
  isEvent,
  isOkRes,
  negotiateMinor,
  parseProtoVersion,
  type ClientKind,
  type ControlMsg,
  type ErrRes,
  type ErrorBody,
  type ErrorCode,
  type Ev,
  type Hello,
  type HelloAck,
  type OkRes,
  type Reject,
  type RejectReason,
  type Req,
  type ReqParams,
  type RequestType,
  type ResOk,
  type ResultMap,
  type Selection,
  type Session,
  type Signal,
  type SpawnSpec,
  type SurfaceMeta,
  type SurfaceState,
  type Tab,
  type ViewState,
  type Workspace,
  type WorkspaceSnapshot,
} from './index.js';

/** Compile-time assertion helpers. */
type Expect<T extends true> = T;
type Equal<A, B> = (<G>() => G extends A ? 1 : 2) extends <G>() => G extends B ? 1 : 2
  ? true
  : false;

/* ------------------------------------------------------------ handshake -- */

const hello: Hello = {
  t: 'hello',
  proto_version: PROTO_VERSION_STRING,
  client_kind: 'control',
  build_id: 'deadbeef-dirty',
};
const helloAck: HelloAck = {
  t: 'hello.ack',
  proto_version: '1.0',
  server_build_id: 'cafebabe',
  workspace_revision: 41,
  server_pid: 1234,
};
const reject: Reject = {
  t: 'reject',
  reason: 'major_mismatch',
  message: 'server speaks 2.0',
  server_version: '2.0',
};
const clientKinds: ClientKind[] = ['control', 'data', 'tool'];
const rejectReasons: RejectReason[] = [
  'major_mismatch',
  'bad_magic',
  'line_too_long',
  'frame_too_large',
  'not_hello',
  'shutting_down',
];

test('handshake messages are constructible', () => {
  expect(hello.t).toBe('hello');
  expect(helloAck.t).toBe('hello.ack');
  expect(reject.t).toBe('reject');
  expect(clientKinds).toHaveLength(3);
  expect(rejectReasons).toHaveLength(6);
});

test('proto version round-trips through its wire string', () => {
  expect(PROTO_VERSION).toEqual({ major: 1, minor: 0 });
  expect(PROTO_VERSION_STRING).toBe('1.0');
  expect(parseProtoVersion('1.0')).toEqual({ major: 1, minor: 0 });
  expect(parseProtoVersion(' 12.34 ')).toEqual({ major: 12, minor: 34 });
  expect(parseProtoVersion('nope')).toBeNull();
  expect(parseProtoVersion('1')).toBeNull();
  expect(formatProtoVersion({ major: 3, minor: 7 })).toBe('3.7');
  expect(negotiateMinor(0, 4)).toBe(0);
  expect(negotiateMinor(9, 4)).toBe(4);
});

/* ------------------------------------------------------------- workspace -- */

const selection: Selection = {
  kind: 'normal',
  anchor: { line: 10342, col: 0 },
  head: { line: 10343, col: 17 },
};
const selectionKinds: Selection['kind'][] = ['normal', 'block', 'lines'];
const viewState: ViewState = { scroll_offset: 0, selection };
const states: SurfaceState[] = [
  { kind: 'running' },
  { kind: 'exited', code: 130, signal: null },
  { kind: 'exited', code: null, signal: 'SIGSEGV' },
];
const surface: SurfaceMeta = {
  id: 9,
  title: 'zsh',
  user_title: null,
  cwd: '/home/sonny',
  cols: 200,
  rows: 60,
  state: { kind: 'running' },
  view_state: viewState,
  has_foreground_child: false,
};
const tab: Tab = { id: 12, surface: 9 };
const session: Session = { id: 1, name: 'Default', active_tab: 12, tabs: [tab] };
const workspace: Workspace = { revision: 42, active_session: 1, sessions: [session] };
const snapshot: WorkspaceSnapshot = { workspace, surfaces: [surface] };

test('workspace document is constructible', () => {
  expect(snapshot.workspace.sessions[0]!.name).toBe('Default');
  expect(selectionKinds).toHaveLength(3);
  expect(states.map((s) => s.kind)).toEqual(['running', 'exited', 'exited']);
  // Q48: cwd and has_foreground_child ride on the surface record.
  expect(surface.has_foreground_child).toBe(false);
  expect(surface.cwd).toBe('/home/sonny');
});

/* -------------------------------------------------------------- requests -- */

const spawn: SpawnSpec = {
  shell: ['/bin/zsh', '-l'],
  cwd: '/tmp',
  env: { LANG: 'en_US.UTF-8' },
  cols: 200,
  rows: 60,
};
const signals: Signal[] = ['HUP', 'TERM', 'KILL'];

/** One value per member of `Req`. */
const requests: Req[] = [
  { t: 'workspace.get', id: 1 },
  { t: 'workspace.subscribe', id: 2 },
  { t: 'session.create', id: 3, name: 'Default' },
  { t: 'session.create', id: 4, name: 'work', if_revision: 41 },
  { t: 'session.rename', id: 5, session: 1, name: 'renamed', if_revision: 41 },
  { t: 'session.delete', id: 6, session: 1 },
  { t: 'session.list', id: 7 },
  { t: 'session.set_active', id: 8, session: 1 },
  { t: 'tab.create', id: 9, session: 1, spawn },
  { t: 'tab.create', id: 10, session: 1, index: 2, surface: 9, if_revision: 41 },
  { t: 'tab.close', id: 11, tab: 12 },
  { t: 'tab.reorder', id: 12, tab: 12, index: 0 },
  { t: 'tab.move', id: 13, tab: 12, to_session: 2, index: 1 },
  { t: 'tab.set_active', id: 14, tab: 12 },
  { t: 'surface.create', id: 15, spawn },
  { t: 'surface.kill', id: 16, surface: 9, signal: 'TERM' },
  { t: 'surface.rename', id: 17, surface: 9, user_title: 'build' },
  { t: 'surface.rename', id: 18, surface: 9, user_title: null },
  { t: 'view.set', id: 19, surface: 9, scroll_offset: 12, selection: null },
  { t: 'server.status', id: 20 },
  { t: 'server.shutdown', id: 21, force: true },
];

/** Every request tag, spelled out; `Equal` proves the list is complete. */
const requestTypes = [
  'workspace.get',
  'workspace.subscribe',
  'session.create',
  'session.rename',
  'session.delete',
  'session.list',
  'session.set_active',
  'tab.create',
  'tab.close',
  'tab.reorder',
  'tab.move',
  'tab.set_active',
  'surface.create',
  'surface.kill',
  'surface.rename',
  'view.set',
  'server.status',
  'server.shutdown',
] as const;

type _RequestTypesAreExhaustive = Expect<Equal<(typeof requestTypes)[number], RequestType>>;
type _ResultMapCoversEveryRequest = Expect<Equal<keyof ResultMap, RequestType>>;

test('every request variant is constructible and every tag is covered', () => {
  expect(new Set(requests.map((r) => r.t)).size).toBe(requestTypes.length);
  for (const t of requestTypes) {
    expect(requests.some((r) => r.t === t)).toBe(true);
  }
  expect(signals).toHaveLength(3);
});

/* ------------------------------------------------------- helper generics -- */

type _TabCreateParams = Expect<
  Equal<
    ReqParams<'tab.create'>,
    {
      session: number;
      index?: number;
      spawn?: SpawnSpec;
      surface?: number;
      if_revision?: number;
    }
  >
>;
type _TabCreateResult = Expect<
  Equal<ResOk<'tab.create'>, { tab: number; surface: number; revision: number }>
>;
type _WorkspaceGetResult = Expect<Equal<ResOk<'workspace.get'>, WorkspaceSnapshot>>;

const tabCreateParams: ReqParams<'tab.create'> = { session: 1, spawn };
const tabCreateResult: ResOk<'tab.create'> = { tab: 12, surface: 9, revision: 42 };

test('helper generics project the right shapes', () => {
  expect(tabCreateParams.session).toBe(1);
  expect(tabCreateResult.surface).toBe(9);
});

/* -------------------------------------------------------------- results -- */

const results: { [K in RequestType]: ResultMap[K] } = {
  'workspace.get': snapshot,
  'workspace.subscribe': snapshot,
  'session.create': { session: 2, revision: 43 },
  'session.rename': { revision: 44 },
  'session.delete': { revision: 45 },
  'session.list': { sessions: [session] },
  'session.set_active': { revision: 46 },
  'tab.create': { tab: 13, surface: 10, revision: 47 },
  'tab.close': { revision: 48 },
  'tab.reorder': { revision: 49 },
  'tab.move': { revision: 50 },
  'tab.set_active': { revision: 51 },
  'surface.create': { surface: 11 },
  'surface.kill': {},
  'surface.rename': { revision: 52 },
  'view.set': { revision: 53 },
  'server.status': {
    build_id: 'cafebabe',
    proto_version: '1.0',
    pid: 1234,
    uptime_s: 60,
    surfaces: 3,
    control_clients: 1,
    data_clients: 1,
    workspace_file: '/home/sonny/.local/state/superterminal/workspace.json',
  },
  'server.shutdown': {},
};

test('every result variant is constructible', () => {
  expect(Object.keys(results).sort()).toEqual([...requestTypes].sort());
});

/* --------------------------------------------------------- envelope/err -- */

const okRes: OkRes<ResultMap['tab.create']> = { t: 'ok', id: 7, result: tabCreateResult };
const errRes: ErrRes = {
  t: 'err',
  id: 9,
  error: { code: 'not_found', message: 'tab 999 does not exist' },
};
const errorBodies: ErrorBody[] = ERROR_CODES.map((code) => ({ code, message: code }));
const errorCodes: ErrorCode[] = [
  'bad_request',
  'not_found',
  'conflict',
  'spawn_failed',
  'unsupported',
  'shutting_down',
  'internal',
];

test('ok/err envelope and every error code', () => {
  expect(isOkRes(okRes)).toBe(true);
  expect(isErrRes(okRes)).toBe(false);
  expect(isErrRes(errRes)).toBe(true);
  expect(errorBodies).toHaveLength(errorCodes.length);
  expect([...ERROR_CODES]).toEqual(errorCodes);
});

/* ---------------------------------------------------------------- events -- */

const events: Ev[] = [
  { t: 'ev.workspace', revision: 42, workspace, surfaces: [surface] },
  { t: 'ev.surface_exited', surface: 9, code: 0, signal: null },
  { t: 'ev.surface_exited', surface: 9, code: null, signal: 'SIGKILL' },
  { t: 'ev.server_shutting_down', reason: 'idle' },
];

type _EvTagsAreExhaustive = Expect<
  Equal<Ev['t'], 'ev.workspace' | 'ev.surface_exited' | 'ev.server_shutting_down'>
>;

test('every event variant is constructible and detected', () => {
  expect(new Set(events.map((e) => e.t)).size).toBe(3);
  for (const e of events) expect(isEvent(e)).toBe(true);
  expect(isEvent(okRes)).toBe(false);
  expect(isEvent(hello)).toBe(false);
});

/* ------------------------------------------------------------ ControlMsg -- */

const anyMessages: ControlMsg[] = [hello, helloAck, reject, okRes, errRes, ...events, ...requests];

test('ControlMsg is the union of everything on the wire', () => {
  expect(anyMessages.length).toBe(5 + events.length + requests.length);
  // Round-trips through JSON unchanged: no class instances, no undefined-only keys.
  for (const m of anyMessages) {
    expect(JSON.parse(JSON.stringify(m))).toEqual(m as unknown as object);
  }
});
