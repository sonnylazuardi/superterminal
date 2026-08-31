import { describe, expect, test } from 'bun:test';
import type { SurfaceMeta, Workspace, WorkspaceSnapshot } from '@superterminal/protocol-ts';
import {
  applyServerEvent,
  applyUiAction,
  initialWorkspaceState,
  snapshotEvent,
} from './reducers.js';
import {
  selectActiveSurface,
  selectActiveTab,
  selectActiveTabs,
  selectMountedSurfaceIds,
  selectRelativeTab,
  selectSessions,
  selectTabAt,
  selectTabIndex,
} from './selectors.js';
import type { WorkspaceState } from './types.js';

/* --------------------------------------------------------------- fixtures -- */

function surface(id: number, over: Partial<SurfaceMeta> = {}): SurfaceMeta {
  return {
    id,
    title: `surface-${id}`,
    user_title: null,
    cwd: '/home/sonny',
    cols: 80,
    rows: 24,
    state: { kind: 'running' },
    view_state: { scroll_offset: 0, selection: null },
    has_foreground_child: false,
    ...over,
  };
}

function workspace(over: Partial<Workspace> = {}): Workspace {
  return {
    revision: 1,
    active_session: 1,
    sessions: [
      {
        id: 1,
        name: 'Default',
        active_tab: 10,
        tabs: [
          { id: 10, surface: 100 },
          { id: 11, surface: 101 },
        ],
      },
    ],
    ...over,
  };
}

function snapshot(over: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return { workspace: workspace(), surfaces: [surface(100), surface(101)], ...over };
}

function seeded(): WorkspaceState {
  return applyServerEvent(initialWorkspaceState, snapshotEvent(snapshot()));
}

/* --------------------------------------------------------------- snapshot -- */

describe('applyServerEvent — snapshot', () => {
  test('projects sessions, tabs and surfaces', () => {
    const s = seeded();
    expect(s.revision).toBe(1);
    expect(s.sessionOrder).toEqual([1]);
    expect(s.sessions[1]).toEqual({ id: 1, name: 'Default', tabIds: [10, 11] });
    expect(s.activeSessionId).toBe(1);
    expect(s.activeTabBySession[1]).toBe(10);
    expect(s.tabs[11]).toEqual({ id: 11, sessionId: 1, surfaceId: 101 });
    expect(s.surfaces[100]).toMatchObject({
      id: 100,
      title: 'surface-100',
      cwd: '/home/sonny',
      status: 'running',
      hasForegroundChild: false,
      bell: false,
    });
  });

  test('the default session is named Default (Q48)', () => {
    expect(seeded().sessions[1]!.name).toBe('Default');
  });

  test('user_title wins over the OSC title', () => {
    const s = applyServerEvent(
      initialWorkspaceState,
      snapshotEvent(snapshot({ surfaces: [surface(100, { user_title: 'build' }), surface(101)] })),
    );
    expect(s.surfaces[100]!.title).toBe('build');
  });

  test('an exited surface projects its code and signal', () => {
    const s = applyServerEvent(
      initialWorkspaceState,
      snapshotEvent(
        snapshot({
          surfaces: [
            surface(100, { state: { kind: 'exited', code: 130, signal: null } }),
            surface(101, { state: { kind: 'exited', code: null, signal: 'SIGKILL' } }),
          ],
        }),
      ),
    );
    expect(s.surfaces[100]).toMatchObject({ status: 'exited', exitCode: 130, exitSignal: null });
    expect(s.surfaces[101]).toMatchObject({ status: 'exited', exitCode: null, exitSignal: 'SIGKILL' });
  });

  test('a null cwd keeps the previous one', () => {
    const before = seeded();
    const after = applyServerEvent(
      before,
      snapshotEvent(snapshot({ surfaces: [surface(100, { cwd: null }), surface(101)] })),
    );
    expect(after.surfaces[100]!.cwd).toBe('/home/sonny');
  });

  test('a session with no active_tab falls back to its first tab', () => {
    const s = applyServerEvent(
      initialWorkspaceState,
      snapshotEvent(
        snapshot({
          workspace: workspace({
            sessions: [
              { id: 1, name: 'Default', active_tab: null, tabs: [{ id: 10, surface: 100 }] },
            ],
          }),
        }),
      ),
    );
    expect(s.activeTabBySession[1]).toBe(10);
  });

  test('an empty workspace yields no active session', () => {
    const s = applyServerEvent(
      initialWorkspaceState,
      snapshotEvent({ workspace: { revision: 3, active_session: 0, sessions: [] }, surfaces: [] }),
    );
    expect(s.activeSessionId).toBeNull();
    expect(s.sessionOrder).toEqual([]);
  });

  test('an unknown active_session falls back to the first session', () => {
    const s = applyServerEvent(
      initialWorkspaceState,
      snapshotEvent(snapshot({ workspace: workspace({ active_session: 99 }) })),
    );
    expect(s.activeSessionId).toBe(1);
  });

  test('the bell flag survives a re-projection', () => {
    let s = seeded();
    s = applyUiAction(s, { type: 'surface.bell', surfaceId: 100 });
    expect(s.surfaces[100]!.bell).toBe(true);
    s = applyServerEvent(s, snapshotEvent(snapshot({ workspace: workspace({ revision: 2 }) })));
    expect(s.surfaces[100]!.bell).toBe(true);
  });

  test('a removed session clears an in-flight rename', () => {
    let s = seeded();
    s = applyUiAction(s, { type: 'session.beginRename', sessionId: 1 });
    expect(s.ui.renamingSessionId).toBe(1);
    s = applyServerEvent(
      s,
      snapshotEvent({
        workspace: { revision: 5, active_session: 2, sessions: [] },
        surfaces: [],
      }),
    );
    expect(s.ui.renamingSessionId).toBeNull();
  });

  test('a removed tab clears the close confirmation', () => {
    let s = seeded();
    s = applyUiAction(s, { type: 'tab.confirmClose', tabId: 11 });
    s = applyServerEvent(
      s,
      snapshotEvent(
        snapshot({
          workspace: workspace({
            revision: 2,
            sessions: [{ id: 1, name: 'Default', active_tab: 10, tabs: [{ id: 10, surface: 100 }] }],
          }),
          surfaces: [surface(100)],
        }),
      ),
    );
    expect(s.ui.confirmingCloseTabId).toBeNull();
    expect(s.tabs[11]).toBeUndefined();
    expect(s.surfaces[101]).toBeUndefined();
  });
});

/* ------------------------------------------------------------ ev.workspace -- */

describe('applyServerEvent — ev.workspace', () => {
  test('replaces the whole projection and bumps the revision', () => {
    const before = seeded();
    const after = applyServerEvent(before, {
      t: 'ev.workspace',
      revision: 7,
      workspace: workspace({
        revision: 7,
        active_session: 2,
        sessions: [
          { id: 2, name: 'work', active_tab: 20, tabs: [{ id: 20, surface: 200 }] },
        ],
      }),
      surfaces: [surface(200, { title: 'vim' })],
    });
    expect(after.revision).toBe(7);
    expect(after.activeSessionId).toBe(2);
    expect(after.sessions[1]).toBeUndefined();
    expect(after.surfaces[200]!.title).toBe('vim');
  });

  test('a stale revision is ignored', () => {
    const before = applyServerEvent(seeded(), {
      t: 'ev.workspace',
      revision: 9,
      workspace: workspace({ revision: 9 }),
      surfaces: [surface(100), surface(101)],
    });
    const after = applyServerEvent(before, {
      t: 'ev.workspace',
      revision: 4,
      workspace: workspace({ revision: 4, sessions: [] }),
      surfaces: [],
    });
    expect(after).toBe(before);
  });

  test('the same revision is re-applied (the document is authoritative)', () => {
    const before = seeded();
    const after = applyServerEvent(before, {
      t: 'ev.workspace',
      revision: 1,
      workspace: workspace({ sessions: [{ id: 1, name: 'renamed', active_tab: 10, tabs: [{ id: 10, surface: 100 }] }] }),
      surfaces: [surface(100)],
    });
    expect(after.sessions[1]!.name).toBe('renamed');
  });
});

/* --------------------------------------------------- ev.surface_exited etc -- */

describe('applyServerEvent — surface lifecycle', () => {
  test('marks the surface exited and clears the foreground child', () => {
    let s = applyServerEvent(
      initialWorkspaceState,
      snapshotEvent(
        snapshot({ surfaces: [surface(100, { has_foreground_child: true }), surface(101)] }),
      ),
    );
    s = applyServerEvent(s, { t: 'ev.surface_exited', surface: 100, code: 0, signal: null });
    expect(s.surfaces[100]).toMatchObject({
      status: 'exited',
      exitCode: 0,
      exitSignal: null,
      hasForegroundChild: false,
    });
    expect(s.surfaces[101]!.status).toBe('running');
  });

  test('a signal exit is recorded', () => {
    const s = applyServerEvent(seeded(), {
      t: 'ev.surface_exited',
      surface: 101,
      code: null,
      signal: 'SIGHUP',
    });
    expect(s.surfaces[101]).toMatchObject({ exitCode: null, exitSignal: 'SIGHUP' });
  });

  test('an exit for an unknown surface is a no-op', () => {
    const before = seeded();
    expect(
      applyServerEvent(before, { t: 'ev.surface_exited', surface: 999, code: 0, signal: null }),
    ).toBe(before);
  });

  test('a repeated exit event is a no-op', () => {
    const once = applyServerEvent(seeded(), {
      t: 'ev.surface_exited',
      surface: 100,
      code: 3,
      signal: null,
    });
    expect(
      applyServerEvent(once, { t: 'ev.surface_exited', surface: 100, code: 3, signal: null }),
    ).toBe(once);
  });

  test('server_shutting_down fails the connection with a reason', () => {
    const s = applyServerEvent(seeded(), { t: 'ev.server_shutting_down', reason: 'idle' });
    expect(s.connection.status).toBe('failed');
    expect(s.connection.error).toBe('server shutting down: idle');
    expect(applyServerEvent(s, { t: 'ev.server_shutting_down', reason: 'idle' })).toBe(s);
  });

  test('an unknown ev.* is ignored (minor-version forward compatibility)', () => {
    const before = seeded();
    const unknown = { t: 'ev.future_thing', whatever: 1 } as unknown as Parameters<
      typeof applyServerEvent
    >[1];
    expect(applyServerEvent(before, unknown)).toBe(before);
  });
});

/* ------------------------------------------------------------- ui actions -- */

describe('applyUiAction', () => {
  test('connection.set records status, version and error', () => {
    let s = applyUiAction(initialWorkspaceState, {
      type: 'connection.set',
      status: 'connected',
      serverVersion: '1.0',
      serverBuildId: 'cafe',
    });
    expect(s.connection).toEqual({
      status: 'connected',
      serverVersion: '1.0',
      serverBuildId: 'cafe',
    });
    const same = applyUiAction(s, {
      type: 'connection.set',
      status: 'connected',
      serverVersion: '1.0',
      serverBuildId: 'cafe',
    });
    expect(same).toBe(s);
    s = applyUiAction(s, { type: 'connection.set', status: 'mismatch', error: 'server speaks 2.0' });
    expect(s.connection.status).toBe('mismatch');
    expect(s.connection.serverVersion).toBe('1.0');
    expect(s.connection.error).toBe('server speaks 2.0');
  });

  test('palette open/close/mode/query/index', () => {
    let s = applyUiAction(seeded(), { type: 'palette.open', mode: 'commands' });
    expect(s.ui).toMatchObject({ paletteOpen: true, paletteMode: 'commands', paletteQuery: '' });

    s = applyUiAction(s, { type: 'palette.setQuery', query: 'new t' });
    expect(s.ui.paletteQuery).toBe('new t');
    expect(applyUiAction(s, { type: 'palette.setQuery', query: 'new t' })).toBe(s);

    s = applyUiAction(s, { type: 'palette.setIndex', index: 2 });
    expect(s.ui.paletteIndex).toBe(2);

    s = applyUiAction(s, { type: 'palette.setMode', mode: 'sessions' });
    expect(s.ui).toMatchObject({ paletteMode: 'sessions', paletteQuery: '', paletteIndex: 0 });
    expect(applyUiAction(s, { type: 'palette.setMode', mode: 'sessions' })).toBe(s);

    s = applyUiAction(s, { type: 'palette.close' });
    expect(s.ui).toMatchObject({ paletteOpen: false, paletteQuery: '', paletteIndex: 0 });
    expect(applyUiAction(s, { type: 'palette.close' })).toBe(s);
  });

  test('palette.move wraps in both directions and tolerates an empty list', () => {
    let s = applyUiAction(seeded(), { type: 'palette.open' });
    s = applyUiAction(s, { type: 'palette.move', delta: 1, count: 3 });
    expect(s.ui.paletteIndex).toBe(1);
    s = applyUiAction(s, { type: 'palette.move', delta: 2, count: 3 });
    expect(s.ui.paletteIndex).toBe(0);
    s = applyUiAction(s, { type: 'palette.move', delta: -1, count: 3 });
    expect(s.ui.paletteIndex).toBe(2);
    s = applyUiAction(s, { type: 'palette.move', delta: 1, count: 0 });
    expect(s.ui.paletteIndex).toBe(0);
  });

  test('vertical tabs toggle', () => {
    let s = applyUiAction(seeded(), { type: 'ui.toggleVerticalTabs' });
    expect(s.ui.verticalTabs).toBe(true);
    s = applyUiAction(s, { type: 'ui.toggleVerticalTabs' });
    expect(s.ui.verticalTabs).toBe(false);
    s = applyUiAction(s, { type: 'ui.setVerticalTabs', value: true });
    expect(s.ui.verticalTabs).toBe(true);
    expect(applyUiAction(s, { type: 'ui.setVerticalTabs', value: true })).toBe(s);
  });

  test('rename begins only for an existing session', () => {
    const s = seeded();
    expect(applyUiAction(s, { type: 'session.beginRename', sessionId: 42 })).toBe(s);
    const renaming = applyUiAction(s, { type: 'session.beginRename', sessionId: 1 });
    expect(renaming.ui.renamingSessionId).toBe(1);
    expect(applyUiAction(renaming, { type: 'session.endRename' }).ui.renamingSessionId).toBeNull();
    expect(applyUiAction(s, { type: 'session.endRename' })).toBe(s);
  });

  test('close confirmation targets an existing tab', () => {
    const s = seeded();
    expect(applyUiAction(s, { type: 'tab.confirmClose', tabId: 999 })).toBe(s);
    const confirming = applyUiAction(s, { type: 'tab.confirmClose', tabId: 10 });
    expect(confirming.ui.confirmingCloseTabId).toBe(10);
    expect(applyUiAction(confirming, { type: 'tab.confirmClose', tabId: null }).ui.confirmingCloseTabId).toBeNull();
  });

  test('bell is set and cleared per surface', () => {
    const s = seeded();
    const rung = applyUiAction(s, { type: 'surface.bell', surfaceId: 100 });
    expect(rung.surfaces[100]!.bell).toBe(true);
    expect(rung.surfaces[101]!.bell).toBe(false);
    expect(applyUiAction(rung, { type: 'surface.bell', surfaceId: 100 })).toBe(rung);
    const cleared = applyUiAction(rung, { type: 'surface.clearBell', surfaceId: 100 });
    expect(cleared.surfaces[100]!.bell).toBe(false);
    expect(applyUiAction(cleared, { type: 'surface.clearBell', surfaceId: 100 })).toBe(cleared);
    expect(applyUiAction(s, { type: 'surface.bell', surfaceId: 999 })).toBe(s);
  });

  test('window resize is recorded once', () => {
    const s = applyUiAction(seeded(), { type: 'window.resize', width: 1200, height: 800 });
    expect(s.ui.window).toEqual({ width: 1200, height: 800 });
    expect(applyUiAction(s, { type: 'window.resize', width: 1200, height: 800 })).toBe(s);
  });

  test('toasts push with increasing ids and dismiss by id', () => {
    let s = applyUiAction(seeded(), { type: 'toast.push', text: 'copied' });
    s = applyUiAction(s, { type: 'toast.push', text: 'boom', kind: 'error' });
    expect(s.ui.toasts).toEqual([
      { id: 1, text: 'copied', kind: 'info' },
      { id: 2, text: 'boom', kind: 'error' },
    ]);
    s = applyUiAction(s, { type: 'toast.dismiss', id: 1 });
    expect(s.ui.toasts.map((t) => t.id)).toEqual([2]);
    expect(applyUiAction(s, { type: 'toast.dismiss', id: 1 })).toBe(s);
  });

  test('ui actions never touch server-owned state', () => {
    const s = seeded();
    const after = applyUiAction(s, { type: 'palette.open' });
    expect(after.sessions).toBe(s.sessions);
    expect(after.tabs).toBe(s.tabs);
    expect(after.surfaces).toBe(s.surfaces);
    expect(after.revision).toBe(s.revision);
  });

  test('server events never touch ui-only state', () => {
    const s = applyUiAction(seeded(), { type: 'ui.toggleVerticalTabs' });
    const after = applyServerEvent(s, {
      t: 'ev.workspace',
      revision: 2,
      workspace: workspace({ revision: 2 }),
      surfaces: [surface(100), surface(101)],
    });
    expect(after.ui.verticalTabs).toBe(true);
    expect(after.ui).toBe(s.ui);
  });
});

/* -------------------------------------------------------------- selectors -- */

describe('selectors', () => {
  test('project the active session, tabs and surface', () => {
    const s = seeded();
    expect(selectSessions(s).map((x) => x.id)).toEqual([1]);
    expect(selectActiveTabs(s).map((t) => t.id)).toEqual([10, 11]);
    expect(selectActiveTab(s)!.id).toBe(10);
    expect(selectActiveSurface(s)!.id).toBe(100);
    expect(selectTabIndex(s, 11)).toBe(1);
    expect(selectTabAt(s, 1)!.id).toBe(11);
    expect(selectTabAt(s, 9)).toBeNull();
  });

  test('memoised per state reference', () => {
    const s = seeded();
    expect(selectActiveTabs(s)).toBe(selectActiveTabs(s));
    const next = applyUiAction(s, { type: 'palette.open' });
    expect(selectActiveTabs(next)).not.toBe(selectActiveTabs(s));
  });

  test('Q44: exactly one surface is mounted', () => {
    const s = seeded();
    expect(selectMountedSurfaceIds(s)).toEqual([100]);
    expect(selectMountedSurfaceIds(initialWorkspaceState)).toEqual([]);
  });

  test('relative tab wraps', () => {
    const s = seeded();
    expect(selectRelativeTab(s, 1)!.id).toBe(11);
    expect(selectRelativeTab(s, 2)!.id).toBe(10);
    expect(selectRelativeTab(s, -1)!.id).toBe(11);
    expect(selectRelativeTab(initialWorkspaceState, 1)).toBeNull();
  });
});
