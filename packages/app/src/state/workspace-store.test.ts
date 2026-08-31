import { describe, expect, test } from 'bun:test';
import type { WorkspaceSnapshot } from '@superterminal/protocol-ts';
import { createWorkspaceStore, getOrCreateGlobalStore } from './workspace-store.js';

const snapshot: WorkspaceSnapshot = {
  workspace: {
    revision: 1,
    active_session: 1,
    sessions: [{ id: 1, name: 'Default', active_tab: 10, tabs: [{ id: 10, surface: 100 }] }],
  },
  surfaces: [
    {
      id: 100,
      title: 'zsh',
      user_title: null,
      cwd: '/tmp',
      cols: 80,
      rows: 24,
      state: { kind: 'running' },
      view_state: { scroll_offset: 0, selection: null },
      has_foreground_child: false,
    },
  ],
};

describe('createWorkspaceStore', () => {
  test('notifies subscribers only when the state object changes', () => {
    const store = createWorkspaceStore();
    let calls = 0;
    const unsubscribe = store.subscribe(() => {
      calls++;
    });
    store.applySnapshot(snapshot);
    expect(calls).toBe(1);
    // A no-op reducer result must not notify (useSyncExternalStore would loop).
    store.dispatch({ type: 'palette.close' });
    expect(calls).toBe(1);
    store.dispatch({ type: 'palette.open' });
    expect(calls).toBe(2);
    unsubscribe();
    store.dispatch({ type: 'palette.close' });
    expect(calls).toBe(2);
    expect(store.listenerCount).toBe(0);
  });

  test('getState is referentially stable between changes', () => {
    const store = createWorkspaceStore();
    const a = store.getState();
    expect(store.getState()).toBe(a);
    store.applySnapshot(snapshot);
    const b = store.getState();
    expect(b).not.toBe(a);
    expect(store.getServerState()).toBe(b);
  });

  test('applyEvent folds control-plane events', () => {
    const store = createWorkspaceStore();
    store.applySnapshot(snapshot);
    store.applyEvent({ t: 'ev.surface_exited', surface: 100, code: 0, signal: null });
    expect(store.getState().surfaces[100]!.status).toBe('exited');
  });

  test('replaceState swaps wholesale and notifies', () => {
    const store = createWorkspaceStore();
    let calls = 0;
    store.subscribe(() => calls++);
    const next = { ...store.getState(), revision: 42 };
    store.replaceState(next);
    expect(store.getState().revision).toBe(42);
    expect(calls).toBe(1);
    store.replaceState(next);
    expect(calls).toBe(1);
  });

  test('a throwing subscriber does not corrupt the state', () => {
    const store = createWorkspaceStore();
    store.subscribe(() => {
      throw new Error('boom');
    });
    expect(() => store.dispatch({ type: 'palette.open' })).toThrow('boom');
    expect(store.getState().ui.paletteOpen).toBe(true);
  });

  test('the global store survives a module re-evaluation', () => {
    const first = getOrCreateGlobalStore();
    first.dispatch({ type: 'ui.setVerticalTabs', value: true });
    const second = getOrCreateGlobalStore();
    expect(second).toBe(first);
    expect(second.getState().ui.verticalTabs).toBe(true);
  });
});
