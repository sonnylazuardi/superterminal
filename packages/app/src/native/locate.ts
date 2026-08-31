/**
 * Locate the superterminal native module and point napi-rs at it.
 *
 * `@gpuix/react` -> `@gpuix/native` -> napi-rs loader, whose `requireNative()`
 * checks `process.env.NAPI_RS_NATIVE_LIBRARY_PATH` first and `require`s that
 * file directly (verified against the installed @gpuix/native 0.6.0
 * `index.js`). Our `.node` is a drop-in superset: it re-exports gpuix's napi
 * classes and additionally registers `TerminalGridFactory` (04 §1.3).
 *
 * This module must run BEFORE `@gpuix/react` is evaluated; ESM imports hoist,
 * so `bunfig.toml` preloads `preload.ts`, which calls `locateNative()`.
 *
 * It never throws. The native module is an optional build artifact: without it
 * the app is still importable (and the whole test suite still runs), it just
 * cannot open a window.
 */

import { existsSync, mkdirSync, copyFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { homedir } from 'node:os';

export const NAPI_ENV_VAR = 'NAPI_RS_NATIVE_LIBRARY_PATH';

export type LocateOutcome =
  | { kind: 'found'; path: string; source: 'env' | 'dev' | 'package' }
  | { kind: 'copied'; path: string; source: 'bunfs'; from: string }
  | { kind: 'missing'; searched: string[] };

export interface LocateEnv {
  env: Record<string, string | undefined>;
  platform: NodeJS.Platform;
  arch: string;
  /** `process.execPath`; used to detect a `bun build --compile` binary. */
  execPath: string;
  /** Directory the repo (or the installed app) lives in. */
  repoRoot: string;
  exists: (p: string) => boolean;
  listNodeFiles: (dir: string) => string[];
  copy: (from: string, to: string) => void;
  log: (message: string) => void;
}

/** napi-rs platform triple, e.g. `linux-x64-gnu`, `darwin-arm64`. */
export function napiTriple(platform: NodeJS.Platform, arch: string): string {
  const cpu = arch === 'x64' ? 'x64' : arch === 'arm64' ? 'arm64' : arch;
  switch (platform) {
    case 'darwin':
      return `darwin-${cpu}`;
    case 'win32':
      return `win32-${cpu}-msvc`;
    default:
      return `${platform}-${cpu}-gnu`;
  }
}

function defaultListNodeFiles(dir: string): string[] {
  try {
    if (!existsSync(dir)) return [];
    return readdirSync(dir)
      .filter((f) => f.endsWith('.node'))
      .map((f) => join(dir, f))
      .filter((p) => {
        try {
          return statSync(p).isFile();
        } catch {
          return false;
        }
      })
      .sort();
  } catch {
    return [];
  }
}

/**
 * Repo root when running from source: this file is
 * `<root>/packages/app/src/native/locate.ts`.
 */
export function defaultRepoRoot(): string {
  return resolve(dirname(new URL(import.meta.url).pathname), '..', '..', '..', '..');
}

export function defaultLocateEnv(overrides: Partial<LocateEnv> = {}): LocateEnv {
  return {
    env: process.env as Record<string, string | undefined>,
    platform: process.platform,
    arch: process.arch,
    execPath: process.execPath,
    repoRoot: defaultRepoRoot(),
    exists: (p) => existsSync(p),
    listNodeFiles: defaultListNodeFiles,
    copy: (from, to) => {
      mkdirSync(dirname(to), { recursive: true });
      copyFileSync(from, to);
    },
    log: (m) => process.stderr.write(`${m}\n`),
    ...overrides,
  };
}

/** `$XDG_CACHE_HOME/superterminal/<build_id>` (05 §10). */
export function cacheDir(env: LocateEnv, buildId: string): string {
  const base = env.env['XDG_CACHE_HOME'] || join(env.env['HOME'] || homedir(), '.cache');
  return join(base, 'superterminal', buildId);
}

/** True for a `bun build --compile` binary (its own name, not `bun`). */
export function isCompiled(env: LocateEnv): boolean {
  const exe = env.execPath.split('/').pop() ?? '';
  return exe !== 'bun' && exe !== 'bun-debug' && exe !== 'node';
}

/**
 * Candidate paths, most specific first.
 *
 * 05 §8 names `packages/native/superterminal-native.<triple>.node` as the dev
 * path; the task brief also names `crates/st-native/target/release/*.node`
 * (where `cargo build` without `napi build` drops it). Both are probed, plus
 * the un-suffixed name a `napi build` without `--platform` produces.
 */
export function candidatePaths(env: LocateEnv): string[] {
  const triple = napiTriple(env.platform, env.arch);
  const root = env.repoRoot;
  const out = [
    join(root, 'packages', 'native', `superterminal-native.${triple}.node`),
    join(root, 'packages', 'native', 'superterminal-native.node'),
  ];
  // Any other .node already sitting in packages/native (e.g. a differently
  // named local build) or in the cargo output directory.
  out.push(...env.listNodeFiles(join(root, 'packages', 'native')));
  out.push(...env.listNodeFiles(join(root, 'crates', 'st-native', 'target', 'release')));
  if (isCompiled(env)) {
    // `bun build --compile --asset packages/native/...` keeps the relative path
    // under the virtual /$bunfs/root tree (05 §10).
    const bunfs = '/$bunfs/root/packages/native';
    out.push(join(bunfs, `superterminal-native.${triple}.node`));
    out.push(...env.listNodeFiles(bunfs));
    // Beside a compiled binary (dist/ layout).
    out.push(join(dirname(env.execPath), `superterminal-native.${triple}.node`));
    out.push(...env.listNodeFiles(dirname(env.execPath)));
  }
  return [...new Set(out)];
}

/**
 * Find the `.node` and set `NAPI_RS_NATIVE_LIBRARY_PATH`.
 *
 * Returns what happened instead of throwing; callers may log or ignore it.
 */
export function locateNative(input: Partial<LocateEnv> = {}): LocateOutcome {
  const env = defaultLocateEnv(input);

  // 1. An explicit override always wins, including one set by a previous call.
  const preset = env.env[NAPI_ENV_VAR];
  if (preset && env.exists(preset)) {
    return { kind: 'found', path: preset, source: 'env' };
  }
  const explicit = env.env['SUPERTERMINAL_NATIVE'];
  if (explicit && env.exists(explicit)) {
    env.env[NAPI_ENV_VAR] = explicit;
    return { kind: 'found', path: explicit, source: 'env' };
  }

  const searched = candidatePaths(env);
  const hit = searched.find((p) => env.exists(p));
  if (!hit) {
    env.log(
      '[superterminal] native module not found; the window cannot open. ' +
        'Build it with `just build-native` (looked in: ' +
        searched.join(', ') +
        ')',
    );
    return { kind: 'missing', searched };
  }

  // 2. Compiled binaries embed the asset under /$bunfs/; dlopen from there is
  //    unverified (05 open question 1), so copy it into the user cache once.
  if (hit.startsWith('/$bunfs/')) {
    const buildId = env.env['SUPERTERMINAL_BUILD_ID'] || 'dev';
    const target = join(cacheDir(env, buildId), 'superterminal-native.node');
    if (!env.exists(target)) {
      try {
        env.copy(hit, target);
      } catch (err) {
        env.log(
          `[superterminal] could not stage the embedded native module at ${target}: ${String(err)}`,
        );
        return { kind: 'missing', searched };
      }
    }
    env.env[NAPI_ENV_VAR] = target;
    return { kind: 'copied', path: target, source: 'bunfs', from: hit };
  }

  env.env[NAPI_ENV_VAR] = hit;
  return { kind: 'found', path: hit, source: hit.includes('/packages/native/') ? 'package' : 'dev' };
}
