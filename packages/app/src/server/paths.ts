/**
 * Socket path resolution.
 *
 * RESOLVED (Q51, docs/plan/00-grilling.md §F): the plan named the socket three
 * ways — `control.sock` (05 §1, 07 M1-08), `server.sock` (02 §1.1) and `sock`
 * (03 §2). `server.sock` wins: Q37 froze a SINGLE socket carrying both planes
 * (distinguished by first-byte sniffing), so a "control" name is misleading, and
 * `crates/st-config` (the shared schema used by the server and the `st` CLI)
 * already resolves `server.sock`. The legacy names stay in the probe list so a
 * daemon built from an older doc is found rather than duplicated. Historic note:
 * M1-08 (the task that creates the listener) agrees with it. `probeCandidates`
 * additionally looks for the other two so a server built to a different doc is
 * still found instead of being silently duplicated. `$SUPERTERMINAL_SOCKET`
 * overrides everything and is what `just dev` sets.
 *
 * RESOLVED (macOS bring-up, 2026-08-31): the two planning docs disagree on the
 * macOS *directory*, and this file used to follow the losing one, which made the
 * client unable to find a daemon on macOS at all —
 *
 *   - 02 §1.1  says `~/Library/Application Support/superterminal/server.sock`
 *   - 03 §2    says `$XDG_RUNTIME_DIR/superterminal/`, falling back to
 *              `$TMPDIR/superterminal-<uid>/`
 *
 * `crates/st-config/src/paths.rs` implements 03 §2 on BOTH platforms, and that
 * is the code the daemon and the `st` CLI actually run: a freshly built
 * `superterminald` on macOS listens at `$TMPDIR/superterminal-<uid>/server.sock`.
 * This module therefore mirrors `Paths::runtime_dir` exactly. Note that
 * `ALTERNATE_SOCKET_FILENAMES` could never have covered this gap: it varies the
 * *file* name, not the directory. The 02 §1.1 location is kept as an extra probe
 * candidate so a daemon that does bind there is still found.
 *
 * Windows is the one platform with no local daemon at all: it lives in WSL and
 * is reached over loopback TCP, so every "socket path" there is a `tcp://`
 * target (see `TCP_ENV_VAR`) and the filesystem branches are only a fallback.
 */

import { homedir, userInfo } from 'node:os';
import { join } from 'node:path';

export const SOCKET_FILENAME = 'server.sock';
export const ALTERNATE_SOCKET_FILENAMES = ['control.sock', 'sock'];
/**
 * When set, the server lives across the Windows/WSL boundary and every
 * socket path in the app becomes a `tcp://host:port` target instead of a
 * filesystem path. A socket file cannot cross the VM boundary; shared
 * localhost TCP can (`superterminald --tcp 127.0.0.1:PORT` in WSL).
 */
export const TCP_ENV_VAR = 'SUPERTERMINAL_TCP';
export const TCP_SCHEME = 'tcp://';

export const APP_DIR = 'superterminal';

export interface PathEnv {
  env?: Record<string, string | undefined>;
  platform?: NodeJS.Platform;
  uid?: number;
}

/** `true` for a `tcp://host:port` target rather than a socket path. */
export function isTcpTarget(target: string): boolean {
  return target.startsWith(TCP_SCHEME);
}

/** Parses `tcp://host:port` into `[hostname, port]`, or `null`. */
export function parseTcpTarget(target: string): [string, number] | null {
  if (!isTcpTarget(target)) return null;
  const rest = target.slice(TCP_SCHEME.length);
  const divider = rest.lastIndexOf(':');
  if (divider <= 0) return null;
  const port = Number(rest.slice(divider + 1));
  if (!Number.isInteger(port) || port <= 0 || port > 65535) return null;
  return [rest.slice(0, divider), port];
}

/** The TCP target from `--tcp` / `$SUPERTERMINAL_TCP`, if one is configured. */
export function tcpTarget(input: PathEnv = {}): string | null {
  const env = input.env ?? process.env;
  const addr = env[TCP_ENV_VAR];
  if (!addr) return null;
  const target = `${TCP_SCHEME}${addr}`;
  return parseTcpTarget(target) ? target : null;
}

/**
 * The runtime directory holding the socket and the lock file.
 *
 * Mirrors `crates/st-config/src/paths.rs::Paths::runtime_dir`, which applies the
 * same order on every platform macOS included — there is deliberately NO
 * `~/Library/Application Support` branch here, because no daemon binds there:
 *
 *   $SUPERTERMINAL_RUNTIME_DIR -> $XDG_RUNTIME_DIR/superterminal
 *                              -> $TMPDIR/superterminal-<uid>
 *                              -> /tmp/superterminal-<uid>
 *
 * Windows is the exception, and only as a fallback: no daemon ever runs beside
 * a Windows client in v1, so this is reached only when `$SUPERTERMINAL_TCP` is
 * unset.
 */
function runtimeDir({ env = process.env, platform = process.platform, uid }: PathEnv): string {
  const override = env['SUPERTERMINAL_RUNTIME_DIR'];
  if (override) return override;

  if (platform === 'win32') {
    const base = env['LOCALAPPDATA'] || env['TEMP'] || env['TMP'] || homedir();
    return join(base, APP_DIR);
  }

  const xdg = env['XDG_RUNTIME_DIR'];
  if (xdg) return join(xdg, APP_DIR);

  const tmp = env['TMPDIR'] || '/tmp';
  const id = uid ?? safeUid();
  return join(tmp, `${APP_DIR}-${id}`);
}

/**
 * The location 02 §1.1 specifies for macOS. Nothing writes here (the daemon
 * follows 03 §2), but it is probed so a daemon that does is found instead of
 * being silently duplicated.
 */
function legacyMacRuntimeDir({ env = process.env }: PathEnv): string {
  return join(env['HOME'] || homedir(), 'Library', 'Application Support', APP_DIR);
}

function safeUid(): number {
  try {
    return userInfo().uid;
  } catch {
    return 0;
  }
}

export function defaultSocketPath(input: PathEnv = {}): string {
  const env = input.env ?? process.env;
  const tcp = tcpTarget({ ...input, env });
  if (tcp) return tcp;
  const override = env['SUPERTERMINAL_SOCKET'];
  if (override) return override;
  return join(runtimeDir(input), SOCKET_FILENAME);
}

/** Every path worth probing before deciding no server is running. */
export function probeCandidates(input: PathEnv = {}): string[] {
  const env = input.env ?? process.env;
  const platform = input.platform ?? process.platform;

  const tcp = tcpTarget({ ...input, env });
  if (tcp) return [tcp];
  const override = env['SUPERTERMINAL_SOCKET'];
  if (override) return [override];

  const dirs = [runtimeDir(input)];
  if (platform === 'darwin') dirs.push(legacyMacRuntimeDir(input));

  const names = [SOCKET_FILENAME, ...ALTERNATE_SOCKET_FILENAMES];
  const out = dirs.flatMap((dir) => names.map((name) => join(dir, name)));
  return [...new Set(out)];
}

/**
 * The state directory holding `workspace.json` and `logs/`.
 *
 * Mirrors `Paths::state_dir`: `$SUPERTERMINAL_STATE_DIR` ->
 * `$XDG_STATE_HOME/superterminal` -> `~/.local/state/superterminal` on Linux,
 * `~/Library/Application Support/superterminal` on macOS (03 §2). On Windows
 * there is no XDG convention and no daemon beside the client, so the client's
 * own state goes where `runtimeDir` already goes: `%LOCALAPPDATA%\superterminal`.
 */
export function stateDir(input: PathEnv = {}): string {
  const env = input.env ?? process.env;
  const platform = input.platform ?? process.platform;

  const override = env['SUPERTERMINAL_STATE_DIR'];
  if (override) return override;

  if (platform === 'win32') {
    const base = env['LOCALAPPDATA'] || env['TEMP'] || env['TMP'] || homedir();
    return join(base, APP_DIR);
  }

  const xdg = env['XDG_STATE_HOME'];
  if (xdg) return join(xdg, APP_DIR);

  const home = env['HOME'] || homedir();
  return platform === 'darwin'
    ? join(home, 'Library', 'Application Support', APP_DIR)
    : join(home, '.local', 'state', APP_DIR);
}
