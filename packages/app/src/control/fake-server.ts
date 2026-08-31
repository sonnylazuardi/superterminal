/**
 * A scripted NDJSON control server over a real Unix socket, for tests (05 §9).
 *
 * Not shipped in the app; it lives beside the client so both share the framing
 * module and drift together.
 */

import { unlinkSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Ev, Hello } from '@superterminal/protocol-ts';
import { NdjsonDecoder, encodeFrame } from './framing.js';

export interface FakeConnection {
  /** Send one already-shaped control message. */
  send(message: unknown): void;
  /** Send raw bytes — used to test partial-chunk framing. */
  sendRaw(bytes: Uint8Array | string): void;
  close(): void;
  readonly received: unknown[];
}

export interface FakeServerOptions {
  /** Reply to `hello` automatically with `hello.ack`. Default true. */
  autoHello?: boolean;
  helloAck?: Partial<{
    proto_version: string;
    server_build_id: string;
    workspace_revision: number;
    server_pid: number;
  }>;
  /** Send this `reject` instead of an ack. */
  reject?: { reason: string; message: string; server_version: string };
  /** Called for every non-handshake request. */
  onRequest?: (message: Record<string, unknown>, conn: FakeConnection) => void;
  onConnection?: (conn: FakeConnection) => void;
}

export interface FakeServer {
  readonly socketPath: string;
  readonly connections: FakeConnection[];
  /** Most recent connection, or throws when there is none. */
  latest(): FakeConnection;
  broadcast(event: Ev): void;
  /** Drop every live connection without stopping the listener. */
  dropConnections(): void;
  stop(): void;
}

export async function startFakeServer(options: FakeServerOptions = {}): Promise<FakeServer> {
  const dir = mkdtempSync(join(tmpdir(), 'st-fake-'));
  const socketPath = join(dir, 'control.sock');
  const connections: FakeConnection[] = [];

  const listener = Bun.listen<{ decoder: NdjsonDecoder; conn: FakeConnection }>({
    unix: socketPath,
    socket: {
      open(socket) {
        const conn: FakeConnection = {
          received: [],
          send(message) {
            socket.write(encodeFrame(message));
          },
          sendRaw(bytes) {
            socket.write(typeof bytes === 'string' ? new TextEncoder().encode(bytes) : bytes);
          },
          close() {
            socket.end();
          },
        };
        socket.data = { decoder: new NdjsonDecoder(), conn };
        connections.push(conn);
        options.onConnection?.(conn);
      },
      data(socket, chunk: Uint8Array) {
        const state = socket.data;
        if (!state) return;
        for (const message of state.decoder.push(chunk)) {
          const m = message as Record<string, unknown>;
          state.conn.received.push(m);
          if (m['t'] === 'hello') {
            const hello = m as unknown as Hello;
            if (options.reject) {
              state.conn.send({ t: 'reject', ...options.reject });
              socket.end();
              continue;
            }
            if (options.autoHello ?? true) {
              state.conn.send({
                t: 'hello.ack',
                proto_version: hello.proto_version,
                server_build_id: 'fake',
                workspace_revision: 1,
                server_pid: 4242,
                ...options.helloAck,
              });
            }
            continue;
          }
          options.onRequest?.(m, state.conn);
        }
      },
      close(socket) {
        const conn = socket.data?.conn;
        if (!conn) return;
        const i = connections.indexOf(conn);
        if (i >= 0) connections.splice(i, 1);
      },
    },
  });

  return {
    socketPath,
    connections,
    latest() {
      const c = connections[connections.length - 1];
      if (!c) throw new Error('fake server has no connection');
      return c;
    },
    broadcast(event) {
      for (const c of connections) c.send(event);
    },
    dropConnections() {
      for (const c of [...connections]) c.close();
    },
    stop() {
      for (const c of [...connections]) c.close();
      listener.stop(true);
      try {
        unlinkSync(socketPath);
      } catch {
        /* already gone */
      }
    },
  };
}

/** Poll until `predicate` holds or the budget runs out. */
export async function waitFor(
  predicate: () => boolean,
  { timeoutMs = 2000, stepMs = 5 }: { timeoutMs?: number; stepMs?: number } = {},
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error('waitFor: condition never became true');
    await Bun.sleep(stepMs);
  }
}
