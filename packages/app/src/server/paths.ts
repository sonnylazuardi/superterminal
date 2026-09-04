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

function runtimeDir({ env = process.env, platform = process.platform, uid }: PathEnv): string {
  if (platform === 'win32') {
    // No daemon ever runs beside a Windows client in v1 (it lives in WSL),
    // so this is only a fallback when $SUPERTERMINAL_TCP is unset.
    const base = env['LOCALAPPDATA'] || env['TEMP'] || env['TMP'] || homedir();
    return join(base, 'superterminal');
  }
  if (platform === 'darwin') {
    // 02 §1.1. 03 §2 suggests $TMPDIR instead; if the daemon lands there,
    // $SUPERTERMINAL_SOCKET or ALTERNATE_SOCKET_FILENAMES cover the gap.
    return join(env['HOME'] || homedir(), 'Library', 'Application Support', 'superterminal');
  }
  const xdg = env['XDG_RUNTIME_DIR'];
  if (xdg) return join(xdg, 'superterminal');
  const id = uid ?? safeUid();
  return join('/tmp', `superterminal-${id}`);
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
  const tcp = tcpTarget({ ...input, env });
  if (tcp) return [tcp];
  const override = env['SUPERTERMINAL_SOCKET'];
  if (override) return [override];
  const dir = runtimeDir(input);
  return [SOCKET_FILENAME, ...ALTERNATE_SOCKET_FILENAMES].map((name) => join(dir, name));
}

/** `$XDG_STATE_HOME/superterminal`, else `~/.local/state/superterminal` (03 §2). */
export function stateDir(input: PathEnv = {}): string {
  const env = input.env ?? process.env;
  const base = env['XDG_STATE_HOME'] || join(env['HOME'] || homedir(), '.local', 'state');
  return join(base, 'superterminal');
}
