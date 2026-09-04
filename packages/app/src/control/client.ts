/**
 * Control-plane client (05 §2).
 *
 * One socket speaking newline-delimited JSON (Q14): `Bun.connect({ unix })`
 * at home, `Bun.connect({ hostname, port })` for a `tcp://` target across the
 * Windows/WSL boundary.
 * Transport only: framing, request/response correlation, timeouts, event
 * dispatch and reconnection. It knows nothing about tabs — the store is the
 * only listener in production.
 */

import type {
  ErrorBody,
  Ev,
  Hello,
  HelloAck,
  Reject,
  ReqParams,
  RequestType,
  ResOk,
} from '@superterminal/protocol-ts';
import { PROTO_VERSION_STRING } from '@superterminal/protocol-ts';
import { debug } from '../util/debug.js';
import {
  ControlError,
  DisconnectedError,
  ProtocolError,
  TimeoutError,
  VersionMismatchError,
} from './errors.js';
import { NdjsonDecoder, encodeFrame } from './framing.js';
import { parseTcpTarget } from '../server/paths.js';

const log = debug('st:control');

/* ------------------------------------------------------------ transport -- */

export interface ControlSocket {
  write(bytes: Uint8Array): void;
  close(): void;
}

export interface TransportHandlers {
  onData(bytes: Uint8Array): void;
  onClose(): void;
  onError(error: Error): void;
}

export type ConnectFn = (path: string, handlers: TransportHandlers) => Promise<ControlSocket>;

/** The real transport. Injectable so tests can drive framing without I/O. */
export const bunConnect: ConnectFn = async (path, handlers) => {
  let closed = false;
  const events = {
    data(_s: unknown, data: Uint8Array) {
      handlers.onData(data);
    },
    close() {
      if (closed) return;
      closed = true;
      handlers.onClose();
    },
    error(_s: unknown, error: Error) {
      handlers.onError(error as Error);
    },
    connectError(_s: unknown, error: Error) {
      handlers.onError(error as Error);
    },
    open() {
      /* nothing: the client sends Hello once connect() resolves */
    },
  };
  // `Bun.connect` has separate overloads per transport, so this is two calls
  // rather than one spread: a `tcp://` target is the Windows/WSL transport.
  const tcp = parseTcpTarget(path);
  const socket = tcp
    ? await Bun.connect({ hostname: tcp[0], port: tcp[1], socket: events })
    : await Bun.connect({ unix: path, socket: events });
  return {
    write(bytes) {
      socket.write(bytes);
    },
    close() {
      closed = true;
      socket.end();
    },
  };
};

/* --------------------------------------------------------------- client -- */

export type ConnectionState = 'connecting' | 'connected' | 'reconnecting' | 'closed';

export interface BackoffOptions {
  initialMs: number;
  maxMs: number;
  factor: number;
}

export const DEFAULT_BACKOFF: BackoffOptions = { initialMs: 250, maxMs: 4000, factor: 2 };

export const DEFAULT_TIMEOUT_MS = 5_000;

/** Spawning a shell can be slow on cold disks (05 §2). */
export const TIMEOUT_OVERRIDES: Partial<Record<RequestType, number>> = {
  'tab.create': 15_000,
  'surface.create': 15_000,
};

export interface ControlClientOptions {
  socketPath: string;
  /** git sha + dirty flag; informational (02 §2). */
  buildId?: string;
  clientKind?: 'control' | 'tool';
  defaultTimeoutMs?: number;
  handshakeTimeoutMs?: number;
  /** Reconnect on an unintentional close. Off in most tests. */
  reconnect?: boolean;
  backoff?: Partial<BackoffOptions>;
  /**
   * Called after every 3 consecutive failed reconnects — the server may have
   * exited idle, so the app re-runs `ensureServer` (05 §2).
   */
  onRepeatedFailure?: (consecutiveFailures: number) => void | Promise<void>;
  connect?: ConnectFn;
}

interface Pending {
  type: RequestType;
  resolve: (value: never) => void;
  reject: (err: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

export interface ServerInfo {
  protoVersion: string;
  buildId: string;
  workspaceRevision: number;
  pid: number;
}

export type EventListener = (event: Ev) => void;
export type StateListener = (state: ConnectionState, error?: Error) => void;

export class ControlClient {
  readonly socketPath: string;

  private readonly opts: Required<
    Pick<
      ControlClientOptions,
      'buildId' | 'clientKind' | 'defaultTimeoutMs' | 'handshakeTimeoutMs' | 'reconnect'
    >
  >;
  private readonly backoff: BackoffOptions;
  private readonly connectFn: ConnectFn;
  private readonly onRepeatedFailure: ((n: number) => void | Promise<void>) | undefined;

  private socket: ControlSocket | null = null;
  private decoder = new NdjsonDecoder();
  private pending = new Map<number, Pending>();
  private eventListeners = new Set<EventListener>();
  private stateListeners = new Set<StateListener>();

  private nextId = 1;
  private _state: ConnectionState = 'connecting';
  private _serverInfo: ServerInfo | null = null;
  private consecutiveFailures = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  /** The current attempt has already been torn down; ignore further errors. */
  private torndown = false;
  private handshakeTimer: ReturnType<typeof setTimeout> | null = null;
  private intentionalClose = false;

  /** Resolved when the handshake for the current attempt completes. */
  private connectedWaiters: Array<{ resolve: () => void; reject: (e: Error) => void }> = [];
  private handshakeSettle: { resolve: (i: ServerInfo) => void; reject: (e: Error) => void } | null =
    null;

  constructor(options: ControlClientOptions) {
    this.socketPath = options.socketPath;
    this.opts = {
      buildId: options.buildId ?? 'dev',
      clientKind: options.clientKind ?? 'control',
      defaultTimeoutMs: options.defaultTimeoutMs ?? DEFAULT_TIMEOUT_MS,
      handshakeTimeoutMs: options.handshakeTimeoutMs ?? 5_000,
      reconnect: options.reconnect ?? true,
    };
    this.backoff = { ...DEFAULT_BACKOFF, ...options.backoff };
    this.connectFn = options.connect ?? bunConnect;
    this.onRepeatedFailure = options.onRepeatedFailure;
  }

  get state(): ConnectionState {
    return this._state;
  }

  get serverInfo(): ServerInfo | null {
    return this._serverInfo;
  }

  /** Number of requests awaiting a response. Exposed for tests. */
  get pendingCount(): number {
    return this.pending.size;
  }

  /* ------------------------------------------------------- connection -- */

  /** Connect and complete the Hello handshake. Rejects on refusal. */
  async connect(): Promise<ServerInfo> {
    if (this._state === 'closed') throw new DisconnectedError('client is closed');
    this.setState(this._state === 'reconnecting' ? 'reconnecting' : 'connecting');
    this.torndown = false;
    const handshake = new Promise<ServerInfo>((resolve, reject) => {
      this.handshakeSettle = { resolve, reject };
    });
    // The transport can report the same failure twice (`connectError` plus the
    // rejected connect promise). Attaching a handler here keeps the second one
    // from surfacing as an unhandled rejection; callers still see the first.
    handshake.catch(() => {});

    let socket: ControlSocket;
    try {
      socket = await this.connectFn(this.socketPath, {
        onData: (bytes) => this.onData(bytes),
        onClose: () => this.onTransportClosed(new DisconnectedError('socket closed')),
        onError: (err) => this.onTransportClosed(err),
      });
    } catch (err) {
      const error = err instanceof Error ? err : new Error(String(err));
      this.handshakeSettle = null;
      this.onTransportClosed(error, /* fromConnect */ true);
      throw error;
    }

    this.socket = socket;
    this.decoder.reset();

    const hello: Hello = {
      t: 'hello',
      proto_version: PROTO_VERSION_STRING,
      client_kind: this.opts.clientKind,
      build_id: this.opts.buildId,
    };
    this.writeRaw(hello);

    this.handshakeTimer = setTimeout(() => {
      this.onTransportClosed(new TimeoutError('hello', this.opts.handshakeTimeoutMs));
    }, this.opts.handshakeTimeoutMs);

    return handshake;
  }

  /** Subscribe to server-pushed events. Returns an unsubscribe function. */
  on(listener: EventListener): () => void {
    this.eventListeners.add(listener);
    return () => this.eventListeners.delete(listener);
  }

  onStateChange(listener: StateListener): () => void {
    this.stateListeners.add(listener);
    return () => this.stateListeners.delete(listener);
  }

  /** Send a request and await its correlated response. */
  request<M extends RequestType>(
    type: M,
    params: ReqParams<M>,
    opts: { timeoutMs?: number } = {},
  ): Promise<ResOk<M>> {
    const timeoutMs = opts.timeoutMs ?? TIMEOUT_OVERRIDES[type] ?? this.opts.defaultTimeoutMs;
    return new Promise<ResOk<M>>((resolve, reject) => {
      if (this._state === 'closed') {
        reject(new DisconnectedError('client is closed'));
        return;
      }
      const id = this.nextId++;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new TimeoutError(type, timeoutMs));
      }, timeoutMs);
      this.pending.set(id, {
        type,
        resolve: resolve as (value: never) => void,
        reject,
        timer,
      });

      const send = () => {
        try {
          this.writeRaw({ t: type, id, ...(params as object) });
        } catch (err) {
          this.settle(id, (p) => p.reject(err instanceof Error ? err : new Error(String(err))));
        }
      };

      if (this._state === 'connected') {
        send();
      } else {
        // Queue until the handshake completes; the request's own timeout is
        // already running, so a never-connecting client still rejects.
        this.whenConnected().then(send, (err: Error) => this.settle(id, (p) => p.reject(err)));
      }
    });
  }

  /** Resolves the next time the client reaches `connected`. */
  whenConnected(): Promise<void> {
    if (this._state === 'connected') return Promise.resolve();
    if (this._state === 'closed') return Promise.reject(new DisconnectedError('client is closed'));
    return new Promise<void>((resolve, reject) => {
      this.connectedWaiters.push({ resolve, reject });
    });
  }

  /** Close for good: no reconnect, pending requests rejected. */
  close(): void {
    if (this._state === 'closed') return;
    this.intentionalClose = true;
    this.clearTimers();
    const socket = this.socket;
    this.socket = null;
    try {
      socket?.close();
    } catch {
      /* already gone */
    }
    this.failPending(new DisconnectedError('client closed'));
    this.setState('closed');
    this.resolveHandshake(null, new DisconnectedError('client closed'));
    this.flushWaiters(new DisconnectedError('client closed'));
  }

  /* ---------------------------------------------------------- internals -- */

  private writeRaw(message: unknown): void {
    const socket = this.socket;
    if (!socket) throw new DisconnectedError('not connected');
    socket.write(encodeFrame(message));
  }

  private onData(bytes: Uint8Array): void {
    let messages: unknown[];
    try {
      messages = this.decoder.push(bytes);
    } catch (err) {
      // A framing violation is connection-fatal (05 §2).
      this.onTransportClosed(err instanceof Error ? err : new ProtocolError(String(err)));
      return;
    }
    for (const message of messages) this.dispatch(message);
  }

  private dispatch(message: unknown): void {
    if (typeof message !== 'object' || message === null || !('t' in message)) {
      log('dropping non-message', message);
      return;
    }
    const t = (message as { t: unknown }).t;
    if (typeof t !== 'string') {
      log('dropping message with non-string t');
      return;
    }

    if (t === 'hello.ack') {
      const ack = message as HelloAck;
      this.clearHandshakeTimer();
      this.consecutiveFailures = 0;
      this._serverInfo = {
        protoVersion: ack.proto_version,
        buildId: ack.server_build_id,
        workspaceRevision: ack.workspace_revision,
        pid: ack.server_pid,
      };
      this.setState('connected');
      this.resolveHandshake(this._serverInfo, null);
      this.flushWaiters(null);
      return;
    }

    if (t === 'reject') {
      const rej = message as Reject;
      this.clearHandshakeTimer();
      const err = new VersionMismatchError(String(rej.reason), rej.message, rej.server_version);
      // A refusal is terminal: no amount of reconnecting fixes a major bump.
      this.intentionalClose = true;
      this.resolveHandshake(null, err);
      this.onTransportClosed(err);
      return;
    }

    if (t === 'ok' || t === 'err') {
      const id = (message as { id?: unknown }).id;
      if (typeof id !== 'number') {
        log('response without a numeric id, dropped');
        return;
      }
      const pending = this.pending.get(id);
      if (!pending) {
        log(`response for unknown id ${id}, dropped`);
        return;
      }
      this.pending.delete(id);
      clearTimeout(pending.timer);
      if (t === 'ok') {
        pending.resolve((message as { result?: unknown }).result as never);
      } else {
        pending.reject(new ControlError((message as unknown as { error: ErrorBody }).error));
      }
      return;
    }

    if (t.startsWith('ev.')) {
      const event = message as Ev;
      for (const listener of [...this.eventListeners]) {
        try {
          listener(event);
        } catch (err) {
          log('event listener threw', err);
        }
      }
      return;
    }

    log(`unknown control message ${t}, ignored`);
  }

  private onTransportClosed(error: Error, fromConnect = false): void {
    this.clearHandshakeTimer();
    if (this._state === 'closed') return;
    if (this.torndown) return;
    this.torndown = true;

    const socket = this.socket;
    this.socket = null;
    if (!fromConnect) {
      try {
        socket?.close();
      } catch {
        /* ignore */
      }
    }
    this.decoder.reset();
    this.failPending(error);
    this.resolveHandshake(null, error);

    if (this.intentionalClose || !this.opts.reconnect) {
      this.setState('closed', error);
      this.flushWaiters(error);
      return;
    }

    this.consecutiveFailures += 1;
    this.setState('reconnecting', error);
    const attempt = this.consecutiveFailures;
    if (attempt > 0 && attempt % 3 === 0 && this.onRepeatedFailure) {
      void Promise.resolve(this.onRepeatedFailure(attempt)).catch((err: unknown) =>
        log('onRepeatedFailure threw', err),
      );
    }
    const delay = Math.min(
      this.backoff.maxMs,
      this.backoff.initialMs * this.backoff.factor ** (attempt - 1),
    );
    log(`reconnecting in ${delay} ms (attempt ${attempt}): ${error.message}`);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      if (this._state === 'closed') return;
      void this.connect().catch(() => {
        /* onTransportClosed already scheduled the next attempt */
      });
    }, delay);
    // Never keep the process alive for a retry (05 §1: the window still opens).
    (this.reconnectTimer as unknown as { unref?: () => void }).unref?.();
  }

  private failPending(error: Error): void {
    const pending = [...this.pending.values()];
    this.pending.clear();
    for (const p of pending) {
      clearTimeout(p.timer);
      p.reject(error);
    }
  }

  private settle(id: number, fn: (p: Pending) => void): void {
    const pending = this.pending.get(id);
    if (!pending) return;
    this.pending.delete(id);
    clearTimeout(pending.timer);
    fn(pending);
  }

  private resolveHandshake(info: ServerInfo | null, error: Error | null): void {
    const settle = this.handshakeSettle;
    if (!settle) return;
    this.handshakeSettle = null;
    if (info) settle.resolve(info);
    else settle.reject(error ?? new DisconnectedError());
  }

  private flushWaiters(error: Error | null): void {
    const waiters = this.connectedWaiters;
    this.connectedWaiters = [];
    for (const w of waiters) {
      if (error) w.reject(error);
      else w.resolve();
    }
  }

  private clearHandshakeTimer(): void {
    if (this.handshakeTimer) {
      clearTimeout(this.handshakeTimer);
      this.handshakeTimer = null;
    }
  }

  private clearTimers(): void {
    this.clearHandshakeTimer();
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  private setState(state: ConnectionState, error?: Error): void {
    if (this._state === state) return;
    this._state = state;
    for (const listener of [...this.stateListeners]) {
      try {
        listener(state, error);
      } catch (err) {
        log('state listener threw', err);
      }
    }
  }
}

export function createControlClient(options: ControlClientOptions): ControlClient {
  return new ControlClient(options);
}
