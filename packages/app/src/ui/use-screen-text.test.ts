import { describe, expect, test } from 'bun:test';
import type { WorkspaceSnapshot } from '@superterminal/protocol-ts';
import { createWorkspaceStore } from '../state/workspace-store.js';
import { fetchScreenText, screenTextSurfaceIds, type ScreenTextClient } from './use-screen-text.js';

/** Session 1 is active: tab 10 with two panes, tab 11 with one. Session 2 is elsewhere. */
const snapshot: WorkspaceSnapshot = {
  workspace: {
    revision: 1,
    active_session: 1,
    sessions: [
      {
        id: 1,
        name: 'Work',
        active_tab: 10,
        tabs: [
          {
            id: 10,
            surface: 100,
            layout: {
              kind: 'split' as const,
              axis: 'row' as const,
              ratio: 0.5,
              first: { kind: 'leaf' as const, surface: 100 },
              second: { kind: 'leaf' as const, surface: 103 },
            },
          },
          { id: 11, surface: 101 },
        ],
      },
      { id: 2, name: 'Home', active_tab: 12, tabs: [{ id: 12, surface: 102 }] },
    ],
  },
  surfaces: [100, 101, 102, 103].map((id) => ({
    id,
    title: `s${id}`,
    user_title: null,
    cwd: '/home/sonny',
    cols: 80,
    rows: 24,
    state: { kind: 'running' as const },
    view_state: { scroll_offset: 0, selection: null },
    has_foreground_child: false,
  })),
};

function state() {
  const store = createWorkspaceStore();
  store.applyEvent({ t: 'snapshot', snapshot });
  return store.getState();
}

interface Call {
  type: string;
  params: { surfaces: number[]; max_rows?: number };
}

function fakeClient(
  answer: (params: { surfaces: number[] }) => Promise<{ screens: Array<{ surface: number; lines: string[] }> }>,
): { client: ScreenTextClient; calls: Call[] } {
  const calls: Call[] = [];
  const client: ScreenTextClient = {
    request(type, params) {
      calls.push({ type, params });
      return answer(params);
    },
  };
  return { client, calls };
}

describe('screenTextSurfaceIds', () => {
  test('every pane of every tab of the active session, once each', () => {
    expect(screenTextSurfaceIds(state())).toEqual([100, 103, 101]);
  });

  test('no active session yields nothing to ask for', () => {
    const store = createWorkspaceStore();
    expect(screenTextSurfaceIds(store.getState())).toEqual([]);
  });
});

describe('fetchScreenText', () => {
  test('one request for every surface, keyed by surface id', async () => {
    const { client, calls } = fakeClient(async ({ surfaces }) => ({
      screens: surfaces.map((surface) => ({ surface, lines: [`on ${surface}`] })),
    }));
    const screens = await fetchScreenText(client, screenTextSurfaceIds(state()));
    expect(calls).toHaveLength(1);
    expect(calls[0]?.type).toBe('surface.screen_text');
    expect(calls[0]?.params.surfaces).toEqual([100, 103, 101]);
    expect(screens.get(103)).toEqual(['on 103']);
    expect(screens.size).toBe(3);
  });

  test('surfaces the daemon omits are simply absent', async () => {
    const { client } = fakeClient(async () => ({ screens: [{ surface: 100, lines: ['hello'] }] }));
    const screens = await fetchScreenText(client, [100, 101]);
    expect(screens.get(100)).toEqual(['hello']);
    expect(screens.has(101)).toBe(false);
  });

  test('a daemon that does not know the request degrades to an empty map', async () => {
    const { client } = fakeClient(async () => {
      throw new Error('unknown request type');
    });
    await expect(fetchScreenText(client, [100])).resolves.toEqual(new Map());
  });

  test('nothing to ask about means no request at all', async () => {
    const { client, calls } = fakeClient(async () => ({ screens: [] }));
    expect(await fetchScreenText(client, [])).toEqual(new Map());
    expect(calls).toHaveLength(0);
  });
});
