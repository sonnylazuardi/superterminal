/**
 * Integration: fake server -> ControlClient -> WorkspaceStore -> selectors.
 * No React, no gpuix, no daemon.
 */

import { afterEach, describe, expect, test } from 'bun:test';
import { ControlClient } from './control/client.js';
import { startFakeServer, waitFor, type FakeServer } from './control/fake-server.js';
import { connect, wireClientToStore } from './bootstrap.js';
import { selectActiveSurface, selectActiveTabs } from './state/selectors.js';
import { createWorkspaceStore } from './state/workspace-store.js';

const servers: FakeServer[] = [];
const clients: ControlClient[] = [];

afterEach(() => {
  for (const c of clients.splice(0)) c.close();
  for (const s of servers.splice(0)) s.stop();
});

const snapshot = {
  workspace: {
    revision: 4,
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
    cwd: '/tmp',
    cols: 80,
    rows: 24,
    state: { kind: 'running' as const },
    view_state: { scroll_offset: 0, selection: null },
    has_foreground_child: false,
  })),
};

describe('client → store wiring', () => {
  test('subscribe fills the projection and events keep it current', async () => {
    const server = await startFakeServer({
      onRequest(message, conn) {
        conn.send({ t: 'ok', id: message['id'], result: snapshot });
      },
    });
    servers.push(server);
    const store = createWorkspaceStore();
    const client = new ControlClient({ socketPath: server.socketPath, reconnect: false });
    clients.push(client);
    wireClientToStore(client, store);

    await connect(client, store, { noSpawn: true, socket: server.socketPath });

    const state = store.getState();
    expect(state.connection.status).toBe('connected');
    expect(state.revision).toBe(4);
    expect(selectActiveTabs(state).map((t) => t.id)).toEqual([10, 11]);
    expect(selectActiveSurface(state)!.id).toBe(100);

    server.broadcast({ t: 'ev.surface_exited', surface: 100, code: 3, signal: null });
    await waitFor(() => store.getState().surfaces[100]!.status === 'exited');
    expect(store.getState().surfaces[100]!.exitCode).toBe(3);

    server.broadcast({
      t: 'ev.workspace',
      revision: 5,
      workspace: {
        ...snapshot.workspace,
        revision: 5,
        sessions: [{ id: 1, name: 'renamed', active_tab: 11, tabs: [{ id: 11, surface: 101 }] }],
      },
      surfaces: [snapshot.surfaces[1]!],
    });
    await waitFor(() => store.getState().revision === 5);
    expect(store.getState().sessions[1]!.name).toBe('renamed');
    expect(selectActiveSurface(store.getState())!.id).toBe(101);
  });

  test('an unreachable server leaves a failed connection, not an exception', async () => {
    const store = createWorkspaceStore();
    const client = new ControlClient({
      socketPath: '/tmp/st-not-there-4711.sock',
      reconnect: false,
    });
    clients.push(client);
    wireClientToStore(client, store);
    await connect(client, store, { noSpawn: true, socket: '/tmp/st-not-there-4711.sock' });
    expect(store.getState().connection.status).toBe('failed');
    expect(store.getState().connection.error).toBeTruthy();
  });

  test('a version mismatch shows up as `mismatch`', async () => {
    const server = await startFakeServer({
      reject: { reason: 'major_mismatch', message: 'server speaks 2.0', server_version: '2.0' },
    });
    servers.push(server);
    const store = createWorkspaceStore();
    const client = new ControlClient({ socketPath: server.socketPath, reconnect: false });
    clients.push(client);
    wireClientToStore(client, store);
    await connect(client, store, { noSpawn: true, socket: server.socketPath });
    expect(store.getState().connection.status).toBe('mismatch');
    expect(store.getState().connection.error).toContain('2.0');
  });
});
