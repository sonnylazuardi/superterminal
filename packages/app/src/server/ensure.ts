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
import { dirname, join, win32 } from 'node:path';
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

/** What a spawn returns: enough to log, detach, and notice an early exit. */
export interface SpawnedServer {
  pid: number;
  unref(): void;
  /** Resolves with the exit code once the process is gone (optional). */
  exited?: Promise<number>;
}

/**
 * Windows: the distro to run the daemon in (`wsl.exe -d <name>`); unset means
 * the default distro.
 */
export const WSL_DISTRO_ENV_VAR = 'SUPERTERMINAL_WSL_DISTRO';

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
  spawn?: (bin: string, args?: string[]) => SpawnedServer;
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

function defaultSpawn(bin: string, args: string[] = []): SpawnedServer {
  const proc = Bun.spawn([bin, ...args], {
    stdio: ['ignore', 'ignore', 'ignore'],
    // A new process group so the daemon survives the terminal that launched
    // the client. Confirmed against Bun 1.4's spawn options.
    detached: true,
    // The client is a GUI-subsystem exe on Windows: without this, a console
    // child (wsl.exe) would pop a black window.
    windowsHide: true,
  });
  proc.unref();
  return { pid: proc.pid, unref: () => proc.unref(), exited: proc.exited };
}

/**
 * The command that starts the daemon inside WSL from the Windows client.
 *
 * WSL kills every process of a `wsl.exe` session the moment that `wsl.exe`
 * exits — `--daemonize`, `setsid` and `nohup` all die with it (verified on
 * WSL 2.7). So the daemon is run *attached*, in the foreground of a hidden
 * `wsl.exe` that simply stays alive for as long as the daemon does; a
 * detached child outlives the client on Windows.
 *
 * `$SUPERTERMINAL_SERVER` is the daemon's path *inside* the distro (default:
 * `superterminald` on WSL's default PATH); `$SUPERTERMINAL_WSL_DISTRO` picks
 * the distro.
 *
 * `--no-idle-exit` is deliberate. This daemon is meant to be the *persistent*
 * backend: it must outlive the app being closed and be warm when the app
 * reopens. Without the flag it inherits `[server].idle_exit_minutes` (15 min
 * by default) and quits after the app has been closed — or merely disconnected
 * across a sleep/resume — for that long with no busy Surface. Re-opening then
 * starts a *fresh* daemon, which re-seeds `workspace.json` with brand-new
 * shells: the previous session's running programs and scrollback are gone.
 * That is the "my session suddenly broke" report. The Startup entry
 * (`docs/WINDOWS.md`) already runs the daemon with `--no-idle-exit` for the
 * same reason; the client must spawn it the same way so behaviour does not
 * depend on which one happened to win the port.
 */
export function wslDaemonCommand(
  tcpTarget: string,
  env: Record<string, string | undefined> = process.env as Record<string, string | undefined>,
): { bin: string; args: string[] } | null {
  const tcp = parseTcpTarget(tcpTarget);
  if (!tcp) return null;
  const systemRoot = env['SystemRoot'] ?? env['SYSTEMROOT'];
  const bin = systemRoot ? win32.join(systemRoot, 'System32', 'wsl.exe') : 'wsl.exe';
  const distro = env[WSL_DISTRO_ENV_VAR];
  const daemon = env['SUPERTERMINAL_SERVER'] || SERVER_BINARY;
  return {
    bin,
    args: [
      ...(distro ? ['-d', distro] : []),
      '--exec',
      daemon,
      '--tcp',
      `${tcp[0]}:${tcp[1]}`,
      '--no-idle-exit',
    ],
  };
}

/** One WSL boot at a time per target: repeated reconnects share the wait. */
const inflightWsl = new Map<string, Promise<EnsureServerResult>>();

/** Cold WSL boots take a while; probe for this long after spawning wsl.exe. */
export const WSL_RETRY_FOR_MS = 30_000;
export const WSL_RETRY_EVERY_MS = 500;

async function ensureWslServer(
  tcpTarget: string,
  options: EnsureServerOptions,
  env: Record<string, string | undefined>,
): Promise<EnsureServerResult> {
  const existing = inflightWsl.get(tcpTarget);
  if (existing) return existing;
  const run = (async () => {
    const probe = options.probe ?? probeSocket;
    const probeTimeoutMs = options.probeTimeoutMs ?? 500;
    const sleep = options.sleep ?? defaultSleep;
    const command = wslDaemonCommand(tcpTarget, env);
    if (!command) {
      throw new ServerUnavailableError('not_running', `bad TCP target ${tcpTarget}`, tcpTarget, [
        tcpTarget,
      ]);
    }

    let child: SpawnedServer;
    try {
      child = (options.spawn ?? defaultSpawn)(command.bin, command.args);
      log(`spawned ${command.bin} ${command.args.join(' ')} (pid ${child.pid})`);
    } catch (err) {
      throw new ServerUnavailableError(
        'spawn_failed',
        `could not start ${command.bin}: ${(err as Error).message}`,
        tcpTarget,
        [tcpTarget],
      );
    }

    // wsl.exe stays up while the daemon runs, so an early exit means the
    // daemon could not start (bad path, WSL error) — fail fast with the code.
    let exitCode: number | null = null;
    void child.exited?.then((code) => {
      exitCode = code;
    });

    const retryForMs = options.retryForMs ?? WSL_RETRY_FOR_MS;
    const retryEveryMs = options.retryEveryMs ?? WSL_RETRY_EVERY_MS;
    const attempts = Math.max(1, Math.ceil(retryForMs / retryEveryMs));
    for (let i = 0; i < attempts; i++) {
      if (await probe(tcpTarget, probeTimeoutMs)) {
        return { socketPath: tcpTarget, spawned: true, pid: child.pid };
      }
      if (exitCode !== null) {
        throw new ServerUnavailableError(
          'spawn_failed',
          `${command.bin} exited with code ${exitCode} before ${SERVER_BINARY} answered on ` +
            `${tcpTarget} (daemon: ${command.args[command.args.indexOf('--exec') + 1]}; ` +
            `set $SUPERTERMINAL_SERVER to its path inside WSL)`,
          tcpTarget,
          [tcpTarget],
        );
      }
      await sleep(retryEveryMs);
    }
    throw new ServerUnavailableError(
      'timeout',
      `${SERVER_BINARY} (via ${command.bin}, pid ${child.pid}) did not answer on ${tcpTarget} ` +
        `within ${retryForMs} ms`,
      tcpTarget,
      [tcpTarget],
    );
  })();
  inflightWsl.set(tcpTarget, run);
  try {
    return await run;
  } finally {
    inflightWsl.delete(tcpTarget);
  }
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

  // 1b. A TCP target means the server lives in WSL. On Windows the client
  // boots WSL and starts the daemon itself through a hidden wsl.exe (see
  // `wslDaemonCommand`); anywhere else there is nothing to spawn, so a failed
  // probe is a hard error with the fix attached.
  const only = candidates.length === 1 ? candidates[0] : undefined;
  const tcpTarget = only !== undefined && isTcpTarget(only) ? only : null;
  if (tcpTarget) {
    const platform = options.platform ?? process.platform;
    if (platform === 'win32' && !options.noSpawn) {
      if (isTestEnvironment(env) && !options.allowSpawnInTests) {
        throw new ServerUnavailableError(
          'not_running',
          `no server on ${tcpTarget}; refusing to spawn wsl.exe from a test run`,
          tcpTarget,
          candidates,
        );
      }
      return ensureWslServer(tcpTarget, options, env);
    }
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
