import { describe, expect, test } from 'bun:test';
import type { ReqParams, RequestType, WorkspaceSnapshot } from '@superterminal/protocol-ts';
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

interface Sent {
  type: RequestType;
  params: unknown;
}

function harness(over: { native?: Partial<NativeBridge> } = {}) {
  const sent: Sent[] = [];
  const store: WorkspaceStore = createWorkspaceStore();
  store.applySnapshot(snapshot);
  store.dispatch({ type: 'connection.set', status: 'connected' });
  const client: ControlClientLike = {
    state: 'connected',
    async request<M extends RequestType>(type: M, params: ReqParams<M>) {
      sent.push({ type, params });
      if (type === 'session.create') return { session: 2, revision: 2 } as never;
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
