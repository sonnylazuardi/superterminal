import { afterEach, describe, expect, test } from 'bun:test';
import type { Ev } from '@superterminal/protocol-ts';
import { ControlClient } from './client.js';
import {
  ControlError,
  DisconnectedError,
  TimeoutError,
  VersionMismatchError,
} from './errors.js';
import { startFakeServer, waitFor, type FakeServer } from './fake-server.js';

const servers: FakeServer[] = [];
const clients: ControlClient[] = [];

function track(server: FakeServer): FakeServer {
  servers.push(server);
  return server;
}
function trackClient(client: ControlClient): ControlClient {
  clients.push(client);
  return client;
}

afterEach(() => {
  for (const c of clients.splice(0)) c.close();
  for (const s of servers.splice(0)) s.stop();
});

const snapshot = {
  workspace: {
    revision: 1,
    active_session: 1,
    sessions: [{ id: 1, name: 'Default', active_tab: 1, tabs: [{ id: 1, surface: 1 }] }],
  },
  surfaces: [
    {
      id: 1,
      title: 'zsh',
      user_title: null,
      cwd: '/home/sonny',
      cols: 80,
      rows: 24,
      state: { kind: 'running' as const },
      view_state: { scroll_offset: 0, selection: null },
      has_foreground_child: false,
    },
  ],
};

describe('ControlClient handshake', () => {
  test('sends Hello and resolves with the server info', async () => {
    const server = track(await startFakeServer({ helloAck: { server_pid: 99 } }));
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false, buildId: 'abc123' }),
    );
    const info = await client.connect();
    expect(info.pid).toBe(99);
    expect(info.protoVersion).toBe('1.0');
    expect(client.state).toBe('connected');
    expect(server.latest().received[0]).toMatchObject({
      t: 'hello',
      client_kind: 'control',
      build_id: 'abc123',
      proto_version: '1.0',
    });
  });

  test('a reject closes the client with VersionMismatchError', async () => {
    const server = track(
      await startFakeServer({
        reject: { reason: 'major_mismatch', message: 'server speaks 2.0', server_version: '2.0' },
      }),
    );
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: true }),
    );
    await expect(client.connect()).rejects.toBeInstanceOf(VersionMismatchError);
    expect(client.state).toBe('closed');
  });

  test('a silent server times out the handshake', async () => {
    const server = track(await startFakeServer({ autoHello: false }));
    const client = trackClient(
      new ControlClient({
        socketPath: server.socketPath,
        reconnect: false,
        handshakeTimeoutMs: 60,
      }),
    );
    await expect(client.connect()).rejects.toBeInstanceOf(TimeoutError);
  });

  test('connecting to a path with no listener rejects', async () => {
    const client = trackClient(
      new ControlClient({ socketPath: '/tmp/st-does-not-exist-12345.sock', reconnect: false }),
    );
    await expect(client.connect()).rejects.toBeDefined();
    expect(client.state).toBe('closed');
  });
});

describe('ControlClient request/response correlation', () => {
  test('resolves the matching id even when responses arrive out of order', async () => {
    const pendingIds: number[] = [];
    const server = track(
      await startFakeServer({
        onRequest(message, conn) {
          const id = message['id'] as number;
          pendingIds.push(id);
          if (pendingIds.length === 2) {
            // Answer the second request first.
            conn.send({ t: 'ok', id: pendingIds[1], result: { revision: 22 } });
            conn.send({ t: 'ok', id: pendingIds[0], result: { revision: 11 } });
          }
        },
      }),
    );
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false }),
    );
    await client.connect();
    const first = client.request('tab.close', { tab: 1 });
    const second = client.request('tab.close', { tab: 2 });
    expect(await first).toEqual({ revision: 11 });
    expect(await second).toEqual({ revision: 22 });
    expect(client.pendingCount).toBe(0);
  });

  test('ids are monotonic and requests carry their params', async () => {
    const seen: Record<string, unknown>[] = [];
    const server = track(
      await startFakeServer({
        onRequest(message, conn) {
          seen.push(message);
          conn.send({ t: 'ok', id: message['id'], result: snapshot });
        },
      }),
    );
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false }),
    );
    await client.connect();
    await client.request('workspace.subscribe', {});
    await client.request('tab.create', { session: 1, spawn: { cols: 200, rows: 60 } });
    expect(seen.map((m) => m['t'])).toEqual(['workspace.subscribe', 'tab.create']);
    expect(seen[0]!['id']).toBe(1);
    expect(seen[1]!['id']).toBe(2);
    expect(seen[1]!['spawn']).toEqual({ cols: 200, rows: 60 });
  });

  test('an err response rejects with ControlError carrying the code', async () => {
    const server = track(
      await startFakeServer({
        onRequest(message, conn) {
          conn.send({
            t: 'err',
            id: message['id'],
            error: { code: 'not_found', message: 'tab 999 does not exist' },
          });
        },
      }),
    );
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false }),
    );
    await client.connect();
    const err = (await client.request('tab.close', { tab: 999 }).catch((e) => e)) as ControlError;
    expect(err).toBeInstanceOf(ControlError);
    expect(err.code).toBe('not_found');
    expect(err.message).toBe('tab 999 does not exist');
  });

  test('a request that is never answered rejects with TimeoutError', async () => {
    const server = track(await startFakeServer({ onRequest() {} }));
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false }),
    );
    await client.connect();
    await expect(client.request('workspace.get', {}, { timeoutMs: 40 })).rejects.toBeInstanceOf(
      TimeoutError,
    );
    expect(client.pendingCount).toBe(0);
  });

  test('a late response for a timed-out id is dropped, not thrown', async () => {
    let reply: (() => void) | null = null;
    const server = track(
      await startFakeServer({
        onRequest(message, conn) {
          reply = () => conn.send({ t: 'ok', id: message['id'], result: { revision: 1 } });
        },
      }),
    );
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false }),
    );
    await client.connect();
    await expect(client.request('tab.close', { tab: 1 }, { timeoutMs: 30 })).rejects.toBeInstanceOf(
      TimeoutError,
    );
    await waitFor(() => reply !== null);
    reply!();
    await Bun.sleep(30);
    expect(client.state).toBe('connected');
  });

  test('a response with an unknown id is ignored', async () => {
    const server = track(
      await startFakeServer({
        onRequest(message, conn) {
          conn.send({ t: 'ok', id: 4242, result: {} });
          conn.send({ t: 'ok', id: message['id'], result: { revision: 5 } });
        },
      }),
    );
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false }),
    );
    await client.connect();
    expect(await client.request('tab.close', { tab: 1 })).toEqual({ revision: 5 });
  });

  test('requests issued before the handshake completes are queued', async () => {
    const server = track(
      await startFakeServer({
        onRequest(message, conn) {
          conn.send({ t: 'ok', id: message['id'], result: snapshot });
        },
      }),
    );
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false }),
    );
    const connecting = client.connect();
    const pending = client.request('workspace.subscribe', {});
    await connecting;
    expect(await pending).toEqual(snapshot);
  });

  test('requests after close() reject immediately', async () => {
    const server = track(await startFakeServer());
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false }),
    );
    await client.connect();
    client.close();
    await expect(client.request('workspace.get', {})).rejects.toBeInstanceOf(DisconnectedError);
  });
});

describe('ControlClient framing over a real socket', () => {
  test('reassembles a response split across writes', async () => {
    const server = track(
      await startFakeServer({
        async onRequest(message, conn) {
          const json = JSON.stringify({ t: 'ok', id: message['id'], result: snapshot });
          conn.sendRaw(json.slice(0, 12));
          await Bun.sleep(10);
          conn.sendRaw(json.slice(12, 40));
          await Bun.sleep(10);
          conn.sendRaw(`${json.slice(40)}\n`);
        },
      }),
    );
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false }),
    );
    await client.connect();
    expect(await client.request('workspace.get', {})).toEqual(snapshot);
  });

  test('handles several messages delivered in one write', async () => {
    const events: Ev[] = [];
    const server = track(
      await startFakeServer({
        onRequest(message, conn) {
          const lines = [
            JSON.stringify({ t: 'ev.surface_exited', surface: 1, code: 0, signal: null }),
            JSON.stringify({ t: 'ok', id: message['id'], result: { revision: 3 } }),
            JSON.stringify({ t: 'ev.server_shutting_down', reason: 'idle' }),
          ];
          conn.sendRaw(`${lines.join('\n')}\n`);
        },
      }),
    );
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false }),
    );
    client.on((ev) => events.push(ev));
    await client.connect();
    expect(await client.request('tab.close', { tab: 1 })).toEqual({ revision: 3 });
    await waitFor(() => events.length === 2);
    expect(events.map((e) => e.t)).toEqual(['ev.surface_exited', 'ev.server_shutting_down']);
  });
});

describe('ControlClient events', () => {
  test('dispatches pushed events in arrival order to every listener', async () => {
    const server = track(await startFakeServer());
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false }),
    );
    const a: string[] = [];
    const b: string[] = [];
    client.on((ev) => a.push(ev.t));
    const off = client.on((ev) => b.push(ev.t));
    await client.connect();
    server.broadcast({ t: 'ev.workspace', revision: 2, ...snapshot });
    server.broadcast({ t: 'ev.surface_exited', surface: 1, code: 0, signal: null });
    await waitFor(() => a.length === 2);
    expect(a).toEqual(['ev.workspace', 'ev.surface_exited']);
    expect(b).toEqual(a);
    off();
    server.broadcast({ t: 'ev.server_shutting_down', reason: 'x' });
    await waitFor(() => a.length === 3);
    expect(b).toHaveLength(2);
  });

  test('a throwing listener does not stop the others', async () => {
    const server = track(await startFakeServer());
    const client = trackClient(
      new ControlClient({ socketPath: server.socketPath, reconnect: false }),
    );
    const seen: string[] = [];
    client.on(() => {
      throw new Error('boom');
    });
    client.on((ev) => seen.push(ev.t));
    await client.connect();
    server.broadcast({ t: 'ev.server_shutting_down', reason: 'x' });
    await waitFor(() => seen.length === 1);
    expect(seen).toEqual(['ev.server_shutting_down']);
  });
});

describe('ControlClient reconnect', () => {
  test('reconnects with backoff after the server drops the connection', async () => {
    const server = track(await startFakeServer());
    const client = trackClient(
      new ControlClient({
        socketPath: server.socketPath,
        reconnect: true,
        backoff: { initialMs: 10, maxMs: 40, factor: 2 },
      }),
    );
    const states: string[] = [];
    client.onStateChange((s) => states.push(s));
    await client.connect();
    expect(server.connections).toHaveLength(1);

    server.dropConnections();
    await waitFor(() => client.state === 'connected' && server.connections.length === 1, {
      timeoutMs: 3000,
    });
    expect(states).toContain('reconnecting');
    expect(client.state).toBe('connected');
  });

  test('pending requests are rejected with DisconnectedError on an unexpected close', async () => {
    const server = track(await startFakeServer({ onRequest() {} }));
    const client = trackClient(
      new ControlClient({
        socketPath: server.socketPath,
        reconnect: true,
        backoff: { initialMs: 10, maxMs: 20, factor: 2 },
      }),
    );
    await client.connect();
    const inflight = client.request('workspace.get', {}, { timeoutMs: 5000 });
    await waitFor(() => client.pendingCount === 1);
    server.dropConnections();
    await expect(inflight).rejects.toBeInstanceOf(DisconnectedError);
  });

  test('onRepeatedFailure fires after three consecutive failures', async () => {
    const calls: number[] = [];
    const client = trackClient(
      new ControlClient({
        socketPath: '/tmp/st-never-there-98765.sock',
        reconnect: true,
        backoff: { initialMs: 5, maxMs: 10, factor: 1 },
        onRepeatedFailure: (n) => {
          calls.push(n);
        },
      }),
    );
    await client.connect().catch(() => {});
    await waitFor(() => calls.length >= 1, { timeoutMs: 3000 });
    expect(calls[0]).toBe(3);
    client.close();
  });

  test('close() stops reconnecting', async () => {
    const server = track(await startFakeServer());
    const client = trackClient(
      new ControlClient({
        socketPath: server.socketPath,
        reconnect: true,
        backoff: { initialMs: 5, maxMs: 10, factor: 1 },
      }),
    );
    await client.connect();
    client.close();
    expect(client.state).toBe('closed');
    server.dropConnections();
    await Bun.sleep(60);
    expect(client.state).toBe('closed');
    expect(server.connections).toHaveLength(0);
  });
});
