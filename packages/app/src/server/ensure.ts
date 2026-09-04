/**
 * Make sure a `superterminald` is listening (05 §1 step 3, Q30).
 *
 *   probe -> (spawn detached + unref) -> retry-connect for 3 s -> typed error
 *
 * The daemon handles stale-socket cleanup and the lockfile itself, so a lost
 * spawn race is harmless: the loser logs "already running" and exits 0.
 *
 * DEVIATION from 05 §1 step 2: the successful probe socket is *not* kept as the
 * control connection. `Bun.connect` binds its handlers at connect time, so
 * handing a live socket to `ControlClient` would mean reaching into the client's
 * framing state from outside. A second connect to a Unix socket costs
 * microseconds; the simplicity is worth it. If it ever shows up in a profile,
 * `ControlClient` can grow an `adopt(socket)` entry point.
 */

import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { debug } from '../util/debug.js';
import {
  defaultSocketPath,
  isTcpTarget,
  parseTcpTarget,
  probeCandidates,
  type PathEnv,
} from './paths.js';

const log = debug('st:server');

export type ServerErrorKind =
  | 'not_running' // nothing listening and spawning was disabled
  | 'binary_not_found' // no `superterminald` anywhere we looked
  | 'spawn_failed' // Bun.spawn threw
  | 'timeout'; // spawned, but never accepted a connection

export class ServerUnavailableError extends Error {
  readonly kind: ServerErrorKind;
  readonly socketPath: string;
  readonly searched: string[];
  constructor(kind: ServerErrorKind, message: string, socketPath: string, searched: string[] = []) {
    super(message);
    this.name = 'ServerUnavailableError';
    this.kind = kind;
    this.socketPath = socketPath;
    this.searched = searched;
  }
}

export interface EnsureServerOptions extends PathEnv {
  /** Explicit socket (`--socket`). */
  socketPath?: string;
  /** `--no-spawn`: probe only, never start a daemon. */
  noSpawn?: boolean;
  probeTimeoutMs?: number;
  retryForMs?: number;
  retryEveryMs?: number;
  /** Injectable for tests. */
  probe?: (path: string, timeoutMs: number) => Promise<boolean>;
  spawn?: (bin: string) => { pid: number; unref(): void };
  exists?: (path: string) => boolean;
  which?: (bin: string) => string | null;
  execPath?: string;
  sleep?: (ms: number) => Promise<void>;
  /**
   * Escape hatch for the guard below. `bun test` sets NODE_ENV=test, and a test
   * that accidentally spawns a real daemon would leak a process into the
   * developer's session.
   */
  allowSpawnInTests?: boolean;
}

export interface EnsureServerResult {
  socketPath: string;
  /** True when this call started the daemon. */
  spawned: boolean;
  pid?: number;
}

export const SERVER_BINARY = 'superterminald';

export function isTestEnvironment(env: Record<string, string | undefined> = process.env): boolean {
  return env['NODE_ENV'] === 'test' || Boolean(env['BUN_TEST']) || Boolean(env['VITEST']);
}

/** Can we open the target? Closes immediately; never throws. */
export async function probeSocket(path: string, timeoutMs = 500): Promise<boolean> {
  const tcp = parseTcpTarget(path);
  if (tcp) return probeTcp(tcp[0], tcp[1], timeoutMs);
  let settled = false;
  return await new Promise<boolean>((resolve) => {
    const finish = (ok: boolean) => {
      if (settled) return;
      settled = true;
      resolve(ok);
    };
    const timer = setTimeout(() => finish(false), timeoutMs);
    (timer as unknown as { unref?: () => void }).unref?.();

    Bun.connect({
      unix: path,
      socket: {
        data() {},
        open(socket) {
          socket.end();
        },
        close() {},
        error() {},
        connectError() {},
      },
    })
      .then((socket) => {
        clearTimeout(timer);
        try {
          socket.end();
        } catch {
          /* ignore */
        }
        finish(true);
      })
      .catch(() => {
        clearTimeout(timer);
        finish(false);
      });
  });
}

/** `$SUPERTERMINAL_SERVER`, then beside this binary, then `$PATH` (05 §1). */
export function locateServerBinary(options: EnsureServerOptions = {}): string | null {
  const env = options.env ?? (process.env as Record<string, string | undefined>);
  const exists = options.exists ?? ((p: string) => existsSync(p));
  const which = options.which ?? ((b: string) => Bun.which(b));
  const execPath = options.execPath ?? process.execPath;

  const explicit = env['SUPERTERMINAL_SERVER'];
  if (explicit && exists(explicit)) return explicit;

  const sibling = join(dirname(execPath), SERVER_BINARY);
  if (exists(sibling)) return sibling;

  return which(SERVER_BINARY);
}

function defaultSpawn(bin: string): { pid: number; unref(): void } {
  const proc = Bun.spawn([bin], {
    stdio: ['ignore', 'ignore', 'ignore'],
    // A new process group so the daemon survives the terminal that launched
    // the client. Confirmed against Bun 1.4's spawn options.
    detached: true,
  });
  proc.unref();
  return { pid: proc.pid, unref: () => proc.unref() };
}

const defaultSleep = (ms: number) => Bun.sleep(ms);

async function probeTcp(host: string, port: number, timeoutMs: number): Promise<boolean> {
  let settled = false;
  return await new Promise<boolean>((resolve) => {
    const finish = (ok: boolean) => {
      if (settled) return;
      settled = true;
      resolve(ok);
    };
    const timer = setTimeout(() => finish(false), timeoutMs);
    (timer as unknown as { unref?: () => void }).unref?.();

    Bun.connect({
      hostname: host,
      port,
      socket: {
        data() {},
        open(socket) {
          socket.end();
        },
        close() {},
        error() {},
        connectError() {},
      },
    })
      .then((socket) => {
        clearTimeout(timer);
        try {
          socket.end();
        } catch {
          /* ignore */
        }
        finish(true);
      })
      .catch(() => {
        clearTimeout(timer);
        finish(false);
      });
  });
}

export async function ensureServer(
  options: EnsureServerOptions = {},
): Promise<EnsureServerResult> {
  const env = options.env ?? (process.env as Record<string, string | undefined>);
  const socketPath = options.socketPath ?? defaultSocketPath({ ...options, env });
  const probe = options.probe ?? probeSocket;
  const probeTimeoutMs = options.probeTimeoutMs ?? 500;
  const sleep = options.sleep ?? defaultSleep;

  // 1. Anything already listening? Look for the alternate spellings too.
  const candidates = options.socketPath
    ? [options.socketPath]
    : probeCandidates({ ...options, env });
  for (const candidate of candidates) {
    if (await probe(candidate, probeTimeoutMs)) {
      log(`server already listening on ${candidate}`);
      return { socketPath: candidate, spawned: false };
    }
  }

  // 1b. A TCP target means the server lives in WSL: there is no local binary
  // to spawn (and spawning a Linux daemon from Windows is meaningless), so a
  // failed probe is a hard error with the fix attached.
  const only = candidates.length === 1 ? candidates[0] : undefined;
  const tcpTarget = only !== undefined && isTcpTarget(only) ? only : null;
  if (tcpTarget) {
    throw new ServerUnavailableError(
      'not_running',
      `no server on ${tcpTarget}; start one in WSL first: ` +
        `superterminald --tcp ${tcpTarget.replace('tcp://', '')} (then relaunch)`,
      tcpTarget,
      candidates,
    );
  }

  if (options.noSpawn) {
    throw new ServerUnavailableError(
      'not_running',
      `no server on ${socketPath} and --no-spawn was given`,
      socketPath,
      candidates,
    );
  }

  // 2. Never start a daemon from a test run unless explicitly allowed.
  if (isTestEnvironment(env) && !options.allowSpawnInTests) {
    throw new ServerUnavailableError(
      'not_running',
      `no server on ${socketPath}; refusing to spawn ${SERVER_BINARY} from a test run`,
      socketPath,
      candidates,
    );
  }

  const bin = locateServerBinary({ ...options, env });
  if (!bin) {
    throw new ServerUnavailableError(
      'binary_not_found',
      `could not find ${SERVER_BINARY} (set $SUPERTERMINAL_SERVER, or put it on $PATH)`,
      socketPath,
      candidates,
    );
  }

  let pid: number;
  try {
    const child = (options.spawn ?? defaultSpawn)(bin);
    pid = child.pid;
    log(`spawned ${bin} (pid ${pid})`);
  } catch (err) {
    throw new ServerUnavailableError(
      'spawn_failed',
      `could not start ${bin}: ${(err as Error).message}`,
      socketPath,
      candidates,
    );
  }

  // 3. Retry-connect for 3 s (Q30): readiness is implicit.
  const retryForMs = options.retryForMs ?? 3000;
  const retryEveryMs = options.retryEveryMs ?? 250;
  const attempts = Math.max(1, Math.ceil(retryForMs / retryEveryMs));
  for (let i = 0; i < attempts; i++) {
    if (await probe(socketPath, probeTimeoutMs)) {
      return { socketPath, spawned: true, pid };
    }
    await sleep(retryEveryMs);
  }

  throw new ServerUnavailableError(
    'timeout',
    `${SERVER_BINARY} (pid ${pid}) did not accept a connection on ${socketPath} within ${retryForMs} ms`,
    socketPath,
    candidates,
  );
}
