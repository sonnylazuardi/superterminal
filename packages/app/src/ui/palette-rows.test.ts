import { describe, expect, test } from 'bun:test';
import type { WorkspaceSnapshot } from '@superterminal/protocol-ts';
import { buildRegistry } from '../commands/registry.js';
import { createWorkspaceStore } from '../state/workspace-store.js';
import { aiStatusLine } from './AiSettings.js';
import { initialAiStatus } from '../state/reducers.js';
import { buildRows, tabLabel } from './palette-rows.js';

const snapshot: WorkspaceSnapshot = {
  workspace: {
    revision: 1,
    active_session: 1,
    sessions: [
      { id: 1, name: 'Work', active_tab: 10, tabs: [{ id: 10, surface: 100 }, { id: 11, surface: 101 }] },
      { id: 2, name: 'Home', active_tab: 12, tabs: [{ id: 12, surface: 102 }] },
    ],
  },
  surfaces: [
    { id: 100, title: 'sonny@wsl:~/projects/api', user_title: null, cwd: '/home/sonny/projects/api', cols: 80, rows: 24, state: { kind: 'running' as const }, view_state: { scroll_offset: 0, selection: null }, has_foreground_child: true },
    { id: 101, title: 'frontend', user_title: null, cwd: '/home/sonny/projects/app', cols: 80, rows: 24, state: { kind: 'exited' as const, code: 0, signal: null }, view_state: { scroll_offset: 0, selection: null }, has_foreground_child: false },
    { id: 102, title: 'sonny@wsl:~', user_title: null, cwd: '/home/sonny', cols: 80, rows: 24, state: { kind: 'running' as const }, view_state: { scroll_offset: 0, selection: null }, has_foreground_child: false },
  ],
};

function state() {
  const store = createWorkspaceStore();
  store.applyEvent({ t: 'snapshot', snapshot });
  store.dispatch({ type: 'connection.set', status: 'connected' });
  return store.getState();
}

const registry = buildRegistry({ platform: 'linux' });
const hint = (id: string) => registry.shortcutHint(id);

describe('buildRows (08 Q1/Q2)', () => {
  test('an empty query lists commands first, then the active session tabs', () => {
    const { local, all, candidates } = buildRows({ state: state(), commands: registry.commands, mode: 'all', query: '', shortcutHint: hint });
    const kinds = local.map((r) => r.kind);
    expect(kinds.slice(0, 3)).toEqual(['command', 'command', 'command']);
    expect(local.filter((r) => r.kind === 'tab').map((r) => r.title)).toEqual(['~/projects/api', 'frontend']);
    expect(local.some((r) => r.kind === 'session')).toBe(false);
    // Every command, both tabs and both sessions are candidates for Jev.
    expect(all.has('session:2')).toBe(true);
    expect(candidates.filter((c) => c.kind === 'tab')).toHaveLength(2);
  });

  test('a tab is reachable by title, cwd and position', () => {
    const s = state();
    for (const q of ['front', 'projects/app', 'tab 2']) {
      const { local } = buildRows({ state: s, commands: registry.commands, mode: 'all', query: q, shortcutHint: hint });
      expect(local.some((r) => r.key === 'tab:11')).toBe(true);
    }
  });

  test('sessions appear only when searched for', () => {
    const { local } = buildRows({ state: state(), commands: registry.commands, mode: 'all', query: 'home', shortcutHint: hint });
    expect(local.some((r) => r.key === 'session:2')).toBe(true);
  });

  test('sessions mode lists sessions and offers a new one', () => {
    const { local } = buildRows({ state: state(), commands: registry.commands, mode: 'sessions', query: 'Ops', shortcutHint: hint });
    expect(local.map((r) => r.key)).toEqual(['session-new']);
    const all = buildRows({ state: state(), commands: registry.commands, mode: 'sessions', query: '', shortcutHint: hint });
    expect(all.local.map((r) => r.title)).toEqual(['Work', 'Home']);
  });

  test('the Jev label says what the client knows and nothing more', () => {
    const s = state();
    const tab = s.tabs[10]!;
    const label = tabLabel(s, tab, 0);
    expect(label).toContain('Tab 1');
    expect(label).toContain('projects/api');
    expect(label).toContain('session Work');
    expect(label).toContain('busy');
    expect(label).toContain('active');
    expect(tabLabel(s, s.tabs[11]!, 1)).toContain('exited');
  });
});

describe('aiStatusLine', () => {
  test('one line per state, key shown as its tail only', () => {
    expect(aiStatusLine(initialAiStatus)).toContain('no key found');
    expect(aiStatusLine({ ...initialAiStatus, status: 'ready', source: 'app', last4: 'cdef', lastLatencyMs: 612 })).toBe(
      'AI ranking: on · key from this app · key …cdef · last call 612 ms',
    );
    expect(aiStatusLine({ ...initialAiStatus, status: 'disabled', lastError: 'Model x is not supported' })).toContain('disabled');
    expect(aiStatusLine({ ...initialAiStatus, enabled: false })).toContain('palette = false');
  });
});

describe('buildRows with screen text (08 §H)', () => {
  // Neither word is in any title or cwd; both are on a pane's screen.
  const screenText = new Map<number, string[]>([
    [100, ['$ cargo run', 'mem 412 MiB · ram monitor']],
    [101, ['$ bun run dev', 'baby tracker listening on http://localhost:3000']],
  ]);

  test('a tab is found by a word only its screen shows', () => {
    const { local } = buildRows({
      state: state(),
      commands: registry.commands,
      mode: 'all',
      query: 'baby',
      shortcutHint: hint,
      screenText,
    });
    const row = local.find((r) => r.key === 'tab:11');
    expect(row).toBeDefined();
    // The evidence is in the hint, marked, so the match is not mysterious.
    expect(row?.hint.startsWith('⌕ ')).toBe(true);
    expect(row?.hint.toLowerCase()).toContain('baby');
  });

  test('a title match outranks a content match', () => {
    // "front" is tab 11's title and only appears on tab 10's screen.
    const screens = new Map<number, string[]>([[100, ['frontend build failed']]]);
    const { local } = buildRows({
      state: state(),
      commands: registry.commands,
      mode: 'all',
      query: 'front',
      shortcutHint: hint,
      screenText: screens,
    });
    const byTitle = local.findIndex((r) => r.key === 'tab:11');
    const byContent = local.findIndex((r) => r.key === 'tab:10');
    expect(byTitle).toBeGreaterThanOrEqual(0);
    expect(byContent).toBeGreaterThan(byTitle);
    // A title match keeps its own hint; only content matches carry the marker.
    expect(local[byTitle]?.hint.startsWith('⌕ ')).toBe(false);
  });

  test('the excerpt reaches the model only when screen context is on', () => {
    const s = state();
    const off = buildRows({ state: s, commands: registry.commands, mode: 'all', query: 'baby', shortcutHint: hint, screenText });
    expect(off.all.get('tab:11')?.label).toBe(tabLabel(s, s.tabs[11]!, 1));

    const on = buildRows({
      state: s,
      commands: registry.commands,
      mode: 'all',
      query: 'baby',
      shortcutHint: hint,
      screenText,
      screenContext: true,
    });
    const label = on.all.get('tab:11')?.label ?? '';
    expect(label.startsWith(tabLabel(s, s.tabs[11]!, 1))).toBe(true);
    expect(label).toContain('screen:');
    expect(label).toContain('baby');
    expect(on.candidates.find((c) => c.id === 'tab:11')?.label).toBe(label);
  });

  test('an empty map changes nothing', () => {
    const s = state();
    const args = { state: s, commands: registry.commands, mode: 'all' as const, shortcutHint: hint };
    for (const query of ['', 'front', 'baby']) {
      const without = buildRows({ ...args, query });
      const withEmpty = buildRows({ ...args, query, screenText: new Map(), screenContext: true });
      expect(withEmpty.local.map((r) => r.key)).toEqual(without.local.map((r) => r.key));
      expect(withEmpty.local.map((r) => r.hint)).toEqual(without.local.map((r) => r.hint));
      expect(withEmpty.candidates.map((c) => c.label)).toEqual(without.candidates.map((c) => c.label));
    }
  });
});
