import { describe, expect, test } from 'bun:test';
import type { ReqParams, RequestType, WorkspaceSnapshot } from '@superterminal/protocol-ts';
import { resolveBinding } from '../platform/keys.js';
import type { WorkspaceState } from '../state/types.js';
import { createWorkspaceStore, type WorkspaceStore } from '../state/workspace-store.js';
import { buildRegistry, filterCommands, fuzzyScore, matchKeybinding } from './registry.js';
import type { CommandContext, ControlClientLike, NativeBridge } from './types.js';
import { noopNativeBridge } from './types.js';

/* -------------------------------------------------------------- fixtures -- */

const snapshot: WorkspaceSnapshot = {
  workspace: {
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
  },
  surfaces: [100, 101].map((id) => ({
    id,
    title: `s${id}`,
    user_title: null,
    cwd: '/home/sonny/projects',
    cols: 200,
    rows: 60,
    state: { kind: 'running' as const },
    view_state: { scroll_offset: 0, selection: null },
    has_foreground_child: false,
  })),
};

/** The fixture with Tab 10 split into [100 | 102] (ADR 0009). */
const splitSnapshot: WorkspaceSnapshot = {
  workspace: {
    ...snapshot.workspace,
    sessions: [
      {
        ...snapshot.workspace.sessions[0]!,
        tabs: [
          {
            id: 10,
            surface: 100,
            layout: {
              kind: 'split',
              axis: 'row',
              ratio: 0.5,
              first: { kind: 'leaf', surface: 100 },
              second: { kind: 'leaf', surface: 102 },
            },
          },
          { id: 11, surface: 101 },
        ],
      },
    ],
  },
  surfaces: [...snapshot.surfaces, { ...snapshot.surfaces[0]!, id: 102, title: 's102', cwd: '/tmp' }],
};

function split(many: boolean): WorkspaceState {
  const store = createWorkspaceStore();
  store.applySnapshot(many ? splitSnapshot : snapshot);
  store.dispatch({ type: 'connection.set', status: 'connected' });
  return store.getState();
}

interface Sent {
  type: RequestType;
  params: unknown;
}

function harness(over: { native?: Partial<NativeBridge>; split?: boolean } = {}) {
  const sent: Sent[] = [];
  const store: WorkspaceStore = createWorkspaceStore();
  store.applySnapshot(over.split ? splitSnapshot : snapshot);
  store.dispatch({ type: 'connection.set', status: 'connected' });
  const client: ControlClientLike = {
    state: 'connected',
    async request<M extends RequestType>(type: M, params: ReqParams<M>) {
      sent.push({ type, params });
      if (type === 'session.create') return { session: 2, revision: 2 } as never;
      if (type === 'tab.split') return { tab: 10, surface: 777, revision: 2 } as never;
      return { revision: 2 } as never;
    },
  };
  const quits: number[] = [];
  const reconnects: number[] = [];
  const ctx: CommandContext = {
    store,
    client,
    native: { ...noopNativeBridge, ...over.native },
    app: {
      quit: () => {
        quits.push(1);
      },
      reconnect: () => {
        reconnects.push(1);
      },
    },
    platform: 'linux',
  };
  return { sent, store, ctx, quits, reconnects };
}

const ev = (
  key: string,
  mods: Partial<Record<'cmd' | 'ctrl' | 'alt' | 'shift', boolean>> = {},
) => ({ key, modifiers: { cmd: false, ctrl: false, alt: false, shift: false, ...mods } });

/* --------------------------------------------------------------- table -- */

describe('registry composition', () => {
  test('every v1 command from Q29 is present', () => {
    const registry = buildRegistry({ platform: 'linux' });
    expect(registry.commands.map((c) => c.id)).toEqual([
      'tab.new',
      'tab.close',
      'pane.splitRight',
      'pane.splitDown',
      'pane.close',
      'pane.focusNext',
      'pane.focusPrev',
      'tab.next',
      'tab.prev',
      'tab.goto',
      'session.new',
      'session.switch',
      'session.rename',
      'view.toggleVerticalTabs',
      'edit.copy',
      'edit.paste',
      'surface.clearScrollback',
      'palette.commands',
      'app.reconnect',
      'app.quit',
    ]);
  });

  test('titles match the spec table', () => {
    const registry = buildRegistry({ platform: 'darwin' });
    expect(registry.byId('tab.new')!.title).toBe('New Tab');
    expect(registry.byId('tab.close')!.title).toBe('Close Tab');
    expect(registry.byId('tab.next')!.title).toBe('Next Tab');
    expect(registry.byId('tab.prev')!.title).toBe('Previous Tab');
    expect(registry.byId('session.new')!.title).toBe('New Session');
    expect(registry.byId('session.switch')!.title).toBe('Switch Session…');
    expect(registry.byId('session.rename')!.title).toBe('Rename Session');
    expect(registry.byId('view.toggleVerticalTabs')!.title).toBe('Toggle Vertical Tabs');
    expect(registry.byId('edit.copy')!.title).toBe('Copy');
    expect(registry.byId('edit.paste')!.title).toBe('Paste');
    expect(registry.byId('surface.clearScrollback')!.title).toBe('Clear Scrollback');
    expect(registry.byId('app.reconnect')!.title).toBe('Reconnect');
    expect(registry.byId('app.quit')!.title).toBe('Quit');
  });

  test('tab.goto is hidden from the palette but bound to nine keys', () => {
    const registry = buildRegistry({ platform: 'linux' });
    const goto = registry.byId('tab.goto')!;
    expect(goto.hidden).toBe(true);
    expect(goto.shortcut).toHaveLength(9);
    expect(goto.shortcutArgs).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9]);
    const { ctx } = harness();
    expect(registry.visible(ctx.store.getState()).some((c) => c.id === 'tab.goto')).toBe(false);
  });
});

/* -------------------------------------------------- platform resolution -- */

describe('platform-aware modifiers', () => {
  test('macOS hints use ⌘', () => {
    const registry = buildRegistry({ platform: 'darwin' });
    expect(registry.shortcutHint('tab.new')).toBe('⌘T');
    expect(registry.shortcutHint('tab.close')).toBe('⌘W');
    expect(registry.shortcutHint('palette.commands')).toBe('⇧⌘P');
    expect(registry.shortcutHint('surface.clearScrollback')).toBe('⇧⌘K');
    expect(registry.shortcutHint('app.reconnect')).toBe('');
  });

  test('Linux hints use Ctrl+Shift', () => {
    const registry = buildRegistry({ platform: 'linux' });
    expect(registry.shortcutHint('tab.new')).toBe('Ctrl+Shift+T');
    expect(registry.shortcutHint('session.switch')).toBe('Ctrl+Shift+K');
    expect(registry.shortcutHint('surface.clearScrollback')).toBe('Ctrl+Shift+L');
  });

  test('tab.goto is ⌘1 on macOS and Alt+1 on Linux', () => {
    expect(buildRegistry({ platform: 'darwin' }).shortcutHint('tab.goto')).toBe('⌘1');
    expect(buildRegistry({ platform: 'linux' }).shortcutHint('tab.goto')).toBe('Alt+1');
  });
});

describe('pane commands (ADR 0009)', () => {
  test('bindings are spelled per platform so mod+shift does not collapse onto mod', () => {
    const mac = buildRegistry({ platform: 'darwin' });
    const linux = buildRegistry({ platform: 'linux' });
    expect(mac.shortcutHint('pane.splitRight')).toBe('⌘D');
    expect(mac.shortcutHint('pane.splitDown')).toBe('⇧⌘D');
    expect(mac.shortcutHint('pane.close')).toBe('⇧⌘W');
    expect(linux.shortcutHint('pane.splitRight')).toBe('Ctrl+Shift+D');
    expect(linux.shortcutHint('pane.splitDown')).toBe('Alt+Shift+D');
    expect(linux.shortcutHint('pane.close')).toBe('Alt+Shift+W');
    expect(linux.shortcutHint('pane.focusNext')).toBe('Alt+]');
  });

  test('no two commands share a keystroke on either platform', () => {
    for (const platform of ['darwin', 'linux'] as const) {
      const registry = buildRegistry({ platform });
      const seen = new Map<string, string>();
      for (const command of registry.commands) {
        for (const binding of command.shortcut) {
          const key = JSON.stringify(resolveBinding(binding, platform));
          expect(seen.get(key) ?? command.id).toBe(command.id);
          seen.set(key, command.id);
        }
      }
    }
  });

  test('Split Right/Down split the focused Pane, spawn in its cwd, then focus the new Pane', async () => {
    const { ctx, sent, store } = harness({ split: true });
    store.dispatch({ type: 'pane.focus', tabId: 10, surfaceId: 102 });
    const registry = buildRegistry({ platform: 'linux' });
    await registry.run('pane.splitRight', ctx);
    expect(sent[0]).toEqual({
      type: 'tab.split',
      params: { tab: 10, pane: 102, axis: 'row', spawn: { cwd: '/tmp', cols: 200, rows: 60 } },
    });
    expect(store.getState().ui.focusedPaneByTab[10]).toBe(777);
    await registry.run('pane.splitDown', ctx, 11);
    expect(sent[1]).toEqual({
      type: 'tab.split',
      params: {
        tab: 11,
        pane: 101,
        axis: 'column',
        spawn: { cwd: '/home/sonny/projects', cols: 200, rows: 60 },
      },
    });
  });

  test('Close Pane on a split Tab closes the focused Pane and focuses its sibling', async () => {
    const { ctx, sent, store } = harness({ split: true });
    const registry = buildRegistry({ platform: 'linux' });
    await registry.run('pane.close', ctx);
    expect(sent).toEqual([{ type: 'pane.close', params: { tab: 10, pane: 100 } }]);
    expect(store.getState().ui.focusedPaneByTab[10]).toBe(102);
  });

  test('Close Pane on a single-Pane Tab is Close Tab, confirmation included', async () => {
    const { ctx, sent, store } = harness();
    const registry = buildRegistry({ platform: 'linux' });
    await registry.run('pane.close', ctx);
    expect(sent).toEqual([{ type: 'tab.close', params: { tab: 10 } }]);
    // Busy Pane: first run asks, second run closes.
    store.applySnapshot({
      ...snapshot,
      surfaces: snapshot.surfaces.map((s) => (s.id === 100 ? { ...s, has_foreground_child: true } : s)),
    });
    await registry.run('pane.close', ctx);
    expect(store.getState().ui.confirmingCloseTabId).toBe(10);
    expect(sent).toHaveLength(1);
    await registry.run('pane.close', ctx);
    expect(sent).toHaveLength(2);
    expect(store.getState().ui.confirmingCloseTabId).toBeNull();
  });

  test('Close Tab confirms when ANY Pane has a foreground child', async () => {
    const { ctx, sent, store } = harness({ split: true });
    store.applySnapshot({
      ...splitSnapshot,
      surfaces: splitSnapshot.surfaces.map((s) => (s.id === 102 ? { ...s, has_foreground_child: true } : s)),
    });
    const registry = buildRegistry({ platform: 'linux' });
    await registry.run('tab.close', ctx);
    expect(sent).toHaveLength(0);
    expect(store.getState().ui.confirmingCloseTabId).toBe(10);
  });

  test('Focus Next/Previous Pane cycle the Panes in tree order', async () => {
    const { ctx, store } = harness({ split: true });
    const registry = buildRegistry({ platform: 'linux' });
    await registry.run('pane.focusNext', ctx);
    expect(store.getState().ui.focusedPaneByTab[10]).toBe(102);
    await registry.run('pane.focusNext', ctx);
    expect(store.getState().ui.focusedPaneByTab[10]).toBe(100);
    await registry.run('pane.focusPrev', ctx);
    expect(store.getState().ui.focusedPaneByTab[10]).toBe(102);
  });

  test('focus cycling is enabled only for a split Tab', () => {
    const registry = buildRegistry({ platform: 'linux' });
    const one = split(false);
    const many = split(true);
    expect(registry.isEnabled(registry.byId('pane.focusNext')!, one)).toBe(false);
    expect(registry.isEnabled(registry.byId('pane.focusNext')!, many)).toBe(true);
    expect(registry.isEnabled(registry.byId('pane.splitRight')!, one)).toBe(true);
    expect(registry.isEnabled(registry.byId('pane.close')!, one)).toBe(true);
  });
});

/* ------------------------------------------------------- passthrough list -- */

describe('passthroughShortcuts', () => {
  test('linux list is the flattened, resolved, de-duplicated set', () => {
    const list = buildRegistry({ platform: 'linux' }).passthroughShortcuts;
    expect(list).toContain('ctrl-shift-t');
    expect(list).toContain('ctrl-shift-w');
    expect(list).toContain('ctrl-tab');
    expect(list).toContain('ctrl-shift-tab');
    expect(list).toContain('ctrl-shift-l');
    expect(list).toContain('alt-1');
    expect(list).toContain('alt-9');
    expect(list).not.toContain('ctrl-t'); // plain Ctrl+T stays terminal input
    expect(new Set(list).size).toBe(list.length);
  });

  test('macOS list uses cmd- names', () => {
    const list = buildRegistry({ platform: 'darwin' }).passthroughShortcuts;
    expect(list).toContain('cmd-t');
    expect(list).toContain('cmd-w');
    expect(list).toContain('shift-cmd-p');
    expect(list).toContain('cmd-1');
    expect(list).toContain('ctrl-tab');
  });

  test('a command with no binding contributes nothing', () => {
    const list = buildRegistry({ platform: 'linux' }).passthroughShortcuts;
    expect(list.some((s) => s.endsWith('-reconnect'))).toBe(false);
  });
});

/* ------------------------------------------------------------- matching -- */

describe('matchKeybinding', () => {
  test('matches on the current platform only', () => {
    const linux = buildRegistry({ platform: 'linux' });
    const mac = buildRegistry({ platform: 'darwin' });
    expect(linux.matchKeybinding(ev('t', { ctrl: true, shift: true }))!.command.id).toBe('tab.new');
    expect(linux.matchKeybinding(ev('t', { cmd: true }))).toBeNull();
    expect(mac.matchKeybinding(ev('t', { cmd: true }))!.command.id).toBe('tab.new');
    expect(mac.matchKeybinding(ev('t', { ctrl: true, shift: true }))).toBeNull();
  });

  test('returns the numeric argument for tab.goto', () => {
    const linux = buildRegistry({ platform: 'linux' });
    expect(linux.matchKeybinding(ev('3', { alt: true }))).toEqual({
      command: linux.byId('tab.goto')!,
      arg: 3,
    });
    const mac = buildRegistry({ platform: 'darwin' });
    expect(mac.matchKeybinding(ev('9', { cmd: true }))!.arg).toBe(9);
  });

  test('secondary bindings match too', () => {
    const linux = buildRegistry({ platform: 'linux' });
    expect(linux.matchKeybinding(ev('tab', { ctrl: true }))!.command.id).toBe('tab.next');
    expect(linux.matchKeybinding(ev('tab', { ctrl: true, shift: true }))!.command.id).toBe(
      'tab.prev',
    );
  });

  test('a disabled command does not match when state is supplied', () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { store } = harness();
    // tab.next needs more than one tab.
    expect(registry.matchKeybinding(ev(']', { ctrl: true, shift: true }), store.getState())!.command
      .id).toBe('tab.next');
    store.applySnapshot({
      workspace: {
        revision: 2,
        active_session: 1,
        sessions: [{ id: 1, name: 'Default', active_tab: 10, tabs: [{ id: 10, surface: 100 }] }],
      },
      surfaces: [snapshot.surfaces[0]!],
    });
    expect(registry.matchKeybinding(ev(']', { ctrl: true, shift: true }), store.getState())).toBeNull();
    // Without state, `when` is not consulted.
    expect(registry.matchKeybinding(ev(']', { ctrl: true, shift: true }))).not.toBeNull();
  });

  test('an unbound keystroke matches nothing', () => {
    const registry = buildRegistry({ platform: 'linux' });
    expect(registry.matchKeybinding(ev('z'))).toBeNull();
    expect(registry.matchKeybinding(ev('t', { ctrl: true }))).toBeNull();
  });

  test('the standalone helper agrees with the registry', () => {
    const registry = buildRegistry({ platform: 'darwin' });
    expect(matchKeybinding(registry.commands, ev('w', { cmd: true }), 'darwin')!.command.id).toBe(
      'tab.close',
    );
  });
});

/* ------------------------------------------------------------ overrides -- */

describe('config keybinding overrides', () => {
  test('replace the default binding', () => {
    const registry = buildRegistry({
      platform: 'linux',
      overrides: { 'tab.new': 'alt+t' },
      onWarning: () => {},
    });
    expect(registry.matchKeybinding(ev('t', { alt: true }))!.command.id).toBe('tab.new');
    expect(registry.matchKeybinding(ev('t', { ctrl: true, shift: true }))).toBeNull();
  });

  test('accept several bindings separated by commas', () => {
    const registry = buildRegistry({
      platform: 'linux',
      overrides: { 'tab.new': 'alt+t, ctrl+shift+n' },
      onWarning: () => {},
    });
    expect(registry.matchKeybinding(ev('t', { alt: true }))!.command.id).toBe('tab.new');
    expect(registry.matchKeybinding(ev('n', { ctrl: true, shift: true }))!.command.id).toBe(
      'tab.new',
    );
  });

  test('an empty override unbinds', () => {
    const registry = buildRegistry({
      platform: 'linux',
      overrides: { 'tab.new': '' },
      onWarning: () => {},
    });
    expect(registry.byId('tab.new')!.shortcut).toEqual([]);
    expect(registry.matchKeybinding(ev('t', { ctrl: true, shift: true }))).toBeNull();
  });

  test('a bad override warns and keeps the default', () => {
    const warnings: string[] = [];
    const registry = buildRegistry({
      platform: 'linux',
      overrides: { 'tab.new': 'hyper+t', 'nope.nope': 'mod+z' },
      onWarning: (m) => warnings.push(m),
    });
    expect(registry.matchKeybinding(ev('t', { ctrl: true, shift: true }))!.command.id).toBe(
      'tab.new',
    );
    expect(warnings.some((w) => w.includes('bad keybinding for tab.new'))).toBe(true);
    expect(warnings.some((w) => w.includes('unknown command id'))).toBe(true);
  });
});

/* --------------------------------------------------------------- when -- */

describe('enablement', () => {
  test('connected-only commands hide while disconnected', () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { store } = harness();
    expect(registry.visible(store.getState()).map((c) => c.id)).toContain('tab.new');
    expect(registry.visible(store.getState()).map((c) => c.id)).not.toContain('app.reconnect');
    store.dispatch({ type: 'connection.set', status: 'reconnecting' });
    const ids = registry.visible(store.getState()).map((c) => c.id);
    expect(ids).not.toContain('tab.new');
    expect(ids).toContain('app.reconnect');
  });

  test('tab.next/prev need more than one tab', () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { store } = harness();
    expect(registry.isEnabled(registry.byId('tab.next')!, store.getState())).toBe(true);
    store.applySnapshot({
      workspace: {
        revision: 2,
        active_session: 1,
        sessions: [{ id: 1, name: 'Default', active_tab: 10, tabs: [{ id: 10, surface: 100 }] }],
      },
      surfaces: [snapshot.surfaces[0]!],
    });
    expect(registry.isEnabled(registry.byId('tab.next')!, store.getState())).toBe(false);
  });
});

/* -------------------------------------------------------------- running -- */

describe('command behaviour', () => {
  test('tab.new spawns in the active surface cwd (Q20)', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, sent } = harness();
    await registry.run('tab.new', ctx);
    expect(sent).toEqual([
      {
        type: 'tab.create',
        params: { session: 1, spawn: { cwd: '/home/sonny/projects', cols: 200, rows: 60 } },
      },
    ]);
  });

  test('tab.close closes immediately with no foreground child', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, sent } = harness();
    await registry.run('tab.close', ctx);
    expect(sent).toEqual([{ type: 'tab.close', params: { tab: 10 } }]);
  });

  test('tab.close asks for confirmation while a child is running (Q21/Q48)', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, sent, store } = harness();
    store.applySnapshot({
      ...snapshot,
      workspace: { ...snapshot.workspace, revision: 2 },
      surfaces: [{ ...snapshot.surfaces[0]!, has_foreground_child: true }, snapshot.surfaces[1]!],
    });
    await registry.run('tab.close', ctx);
    expect(sent).toEqual([]);
    expect(store.getState().ui.confirmingCloseTabId).toBe(10);
    // A second invocation confirms.
    await registry.run('tab.close', ctx);
    expect(sent).toEqual([{ type: 'tab.close', params: { tab: 10 } }]);
    expect(store.getState().ui.confirmingCloseTabId).toBeNull();
  });

  test('tab.next / tab.prev wrap through the active session', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, sent } = harness();
    await registry.run('tab.next', ctx);
    await registry.run('tab.prev', ctx);
    expect(sent).toEqual([
      { type: 'tab.set_active', params: { tab: 11 } },
      { type: 'tab.set_active', params: { tab: 11 } },
    ]);
  });

  test('tab.goto activates the nth tab and ignores an out-of-range index', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, sent } = harness();
    await registry.run('tab.goto', ctx, 2);
    await registry.run('tab.goto', ctx, 7);
    expect(sent).toEqual([{ type: 'tab.set_active', params: { tab: 11 } }]);
  });

  test('session.new creates and activates', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, sent } = harness();
    await registry.run('session.new', ctx, 'scratch');
    expect(sent).toEqual([
      { type: 'session.create', params: { name: 'scratch' } },
      { type: 'session.set_active', params: { session: 2 } },
    ]);
  });

  test('session.new falls back to a numbered name', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, sent } = harness();
    await registry.run('session.new', ctx);
    expect(sent[0]).toEqual({ type: 'session.create', params: { name: 'Session 2' } });
  });

  test('session.switch opens the palette in sessions mode', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, store } = harness();
    await registry.run('session.switch', ctx);
    expect(store.getState().ui).toMatchObject({ paletteOpen: true, paletteMode: 'sessions' });
  });

  test('session.rename starts an in-place rename', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, store } = harness();
    await registry.run('session.rename', ctx);
    expect(store.getState().ui.renamingSessionId).toBe(1);
  });

  test('view.toggleVerticalTabs flips the ui flag', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, store } = harness();
    await registry.run('view.toggleVerticalTabs', ctx);
    expect(store.getState().ui.verticalTabs).toBe(true);
  });

  test('copy / paste / clear scrollback go to the native element', async () => {
    const calls: string[] = [];
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, store } = harness({
      native: {
        copySelection: (id) => {
          calls.push(`copy:${id}`);
          return 'hello';
        },
        paste: (id) => calls.push(`paste:${id}`),
        clearScrollback: (id) => calls.push(`clear:${id}`),
      },
    });
    await registry.run('edit.copy', ctx);
    await registry.run('edit.paste', ctx);
    await registry.run('surface.clearScrollback', ctx);
    expect(calls).toEqual(['copy:100', 'paste:100', 'clear:100']);
    expect(store.getState().ui.toasts.map((t) => t.text)).toEqual(['Copied']);
  });

  test('copy with no selection raises no toast', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, store } = harness({ native: { copySelection: () => null } });
    await registry.run('edit.copy', ctx);
    expect(store.getState().ui.toasts).toEqual([]);
  });

  test('reconnect and quit reach the app bridge', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, quits, reconnects } = harness();
    await registry.run('app.reconnect', ctx);
    await registry.run('app.quit', ctx);
    expect(reconnects).toHaveLength(1);
    expect(quits).toHaveLength(1);
  });

  test('running an unknown id warns instead of throwing', () => {
    const warnings: string[] = [];
    const registry = buildRegistry({ platform: 'linux', onWarning: (m) => warnings.push(m) });
    const { ctx } = harness();
    expect(() => registry.run('nope', ctx)).not.toThrow();
    expect(warnings[0]).toContain('no such command');
  });

  test('commands do nothing without an active session', async () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { ctx, sent, store } = harness();
    store.applySnapshot({
      workspace: { revision: 3, active_session: 0, sessions: [] },
      surfaces: [],
    });
    await registry.run('tab.new', ctx);
    await registry.run('tab.close', ctx);
    await registry.run('edit.paste', ctx);
    expect(sent).toEqual([]);
  });
});

/* --------------------------------------------------------------- fuzzy -- */

describe('fuzzy palette matching', () => {
  test('subsequence matching with word-start and run bonuses', () => {
    expect(fuzzyScore('', 'New Tab')).toBe(0);
    expect(fuzzyScore('nt', 'New Tab')).not.toBeNull();
    expect(fuzzyScore('zz', 'New Tab')).toBeNull();
    expect(fuzzyScore('new', 'New Tab')!).toBeGreaterThan(fuzzyScore('nt', 'New Tab')!);
  });

  test('filterCommands sorts by score and respects when/hidden', () => {
    const registry = buildRegistry({ platform: 'linux' });
    const { store } = harness();
    const results = filterCommands(registry.commands, 'tab', store.getState());
    expect(results.length).toBeGreaterThan(0);
    expect(results.every((r) => !r.command.hidden)).toBe(true);
    expect(results[0]!.command.title.toLowerCase()).toContain('tab');
    expect(filterCommands(registry.commands, 'qqqq', store.getState())).toEqual([]);
  });
});
