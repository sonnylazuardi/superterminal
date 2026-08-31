import { describe, expect, test } from 'bun:test';
import {
  NAPI_ENV_VAR,
  cacheDir,
  candidatePaths,
  defaultLocateEnv,
  isCompiled,
  locateNative,
  napiTriple,
  type LocateEnv,
} from './locate.js';
import { lastOutcome, preloadNative } from './preload.js';

function fakeEnv(over: Partial<LocateEnv> = {}): LocateEnv {
  const logs: string[] = [];
  const base = defaultLocateEnv({
    env: {},
    platform: 'linux',
    arch: 'x64',
    execPath: '/usr/bin/bun',
    repoRoot: '/repo',
    exists: () => false,
    listNodeFiles: () => [],
    copy: () => {
      throw new Error('copy should not be called');
    },
    log: (m) => logs.push(m),
    ...over,
  });
  return Object.assign(base, { logs } as unknown as LocateEnv);
}

function logsOf(env: LocateEnv): string[] {
  return (env as unknown as { logs: string[] }).logs;
}

describe('napiTriple', () => {
  test('matches napi-rs naming', () => {
    expect(napiTriple('linux', 'x64')).toBe('linux-x64-gnu');
    expect(napiTriple('linux', 'arm64')).toBe('linux-arm64-gnu');
    expect(napiTriple('darwin', 'arm64')).toBe('darwin-arm64');
    expect(napiTriple('darwin', 'x64')).toBe('darwin-x64');
    expect(napiTriple('win32', 'x64')).toBe('win32-x64-msvc');
  });
});

describe('isCompiled', () => {
  test('true only for a binary that is not bun/node', () => {
    expect(isCompiled(fakeEnv({ execPath: '/usr/bin/bun' }))).toBe(false);
    expect(isCompiled(fakeEnv({ execPath: '/home/x/.bun/bin/bun-debug' }))).toBe(false);
    expect(isCompiled(fakeEnv({ execPath: '/usr/bin/node' }))).toBe(false);
    expect(isCompiled(fakeEnv({ execPath: '/opt/st/superterminal' }))).toBe(true);
  });
});

describe('candidatePaths', () => {
  test('dev layout probes packages/native and the cargo output dir', () => {
    const env = fakeEnv({
      listNodeFiles: (dir) =>
        dir.endsWith('crates/st-native/target/release')
          ? [`${dir}/libst_native.node`]
          : [],
    });
    const paths = candidatePaths(env);
    expect(paths[0]).toBe('/repo/packages/native/superterminal-native.linux-x64-gnu.node');
    expect(paths).toContain('/repo/packages/native/superterminal-native.node');
    expect(paths).toContain('/repo/crates/st-native/target/release/libst_native.node');
    // Not a compiled binary: no /$bunfs/ probing.
    expect(paths.some((p) => p.startsWith('/$bunfs/'))).toBe(false);
  });

  test('a compiled binary also probes /$bunfs/ and its own directory', () => {
    const env = fakeEnv({ execPath: '/opt/st/superterminal' });
    const paths = candidatePaths(env);
    expect(paths).toContain('/$bunfs/root/packages/native/superterminal-native.linux-x64-gnu.node');
    expect(paths).toContain('/opt/st/superterminal-native.linux-x64-gnu.node');
  });

  test('candidates are de-duplicated', () => {
    const env = fakeEnv({
      listNodeFiles: (dir) =>
        dir.endsWith('packages/native')
          ? [`${dir}/superterminal-native.linux-x64-gnu.node`]
          : [],
    });
    const paths = candidatePaths(env);
    expect(new Set(paths).size).toBe(paths.length);
  });
});

describe('locateNative — missing .node', () => {
  test('does not throw, warns clearly and reports what it looked at', () => {
    const env = fakeEnv();
    const outcome = locateNative(env);
    expect(outcome.kind).toBe('missing');
    expect(env.env[NAPI_ENV_VAR]).toBeUndefined();
    const warning = logsOf(env).join('\n');
    expect(warning).toContain('native module not found');
    expect(warning).toContain('just build-native');
    expect(warning).toContain('packages/native/superterminal-native.linux-x64-gnu.node');
  });

  test('the preload module survives a missing .node and is idempotent', () => {
    // This is the real preload, run against the real (empty) tree.
    const first = preloadNative();
    expect(['missing', 'found', 'copied']).toContain(first.kind);
    expect(preloadNative()).toBe(first);
    expect(lastOutcome()).toBe(first);
  });
});

describe('locateNative — found', () => {
  test('sets NAPI_RS_NATIVE_LIBRARY_PATH to the dev build', () => {
    const hit = '/repo/packages/native/superterminal-native.linux-x64-gnu.node';
    const env = fakeEnv({ exists: (p) => p === hit });
    const outcome = locateNative(env);
    expect(outcome).toEqual({ kind: 'found', path: hit, source: 'package' });
    expect(env.env[NAPI_ENV_VAR]).toBe(hit);
  });

  test('an existing env var is respected untouched', () => {
    const env = fakeEnv({
      env: { [NAPI_ENV_VAR]: '/somewhere/custom.node' },
      exists: (p) => p === '/somewhere/custom.node',
    });
    expect(locateNative(env)).toEqual({
      kind: 'found',
      path: '/somewhere/custom.node',
      source: 'env',
    });
  });

  test('SUPERTERMINAL_NATIVE points the loader at an explicit file', () => {
    const env = fakeEnv({
      env: { SUPERTERMINAL_NATIVE: '/custom/st.node' },
      exists: (p) => p === '/custom/st.node',
    });
    expect(locateNative(env)).toEqual({ kind: 'found', path: '/custom/st.node', source: 'env' });
    expect(env.env[NAPI_ENV_VAR]).toBe('/custom/st.node');
  });

  test('a stale env var falls through to the search', () => {
    const hit = '/repo/packages/native/superterminal-native.linux-x64-gnu.node';
    const env = fakeEnv({
      env: { [NAPI_ENV_VAR]: '/gone.node' },
      exists: (p) => p === hit,
    });
    expect(locateNative(env).kind).toBe('found');
    expect(env.env[NAPI_ENV_VAR]).toBe(hit);
  });
});

describe('locateNative — compiled /$bunfs/ asset', () => {
  const bunfs = '/$bunfs/root/packages/native/superterminal-native.linux-x64-gnu.node';

  test('copies the embedded asset into the cache and points at the copy', () => {
    const copies: Array<[string, string]> = [];
    const env = fakeEnv({
      execPath: '/opt/st/superterminal',
      env: { XDG_CACHE_HOME: '/cache', SUPERTERMINAL_BUILD_ID: 'abc123' },
      exists: (p) => p === bunfs,
      copy: (from, to) => copies.push([from, to]),
    });
    const outcome = locateNative(env);
    expect(outcome).toEqual({
      kind: 'copied',
      source: 'bunfs',
      from: bunfs,
      path: '/cache/superterminal/abc123/superterminal-native.node',
    });
    expect(copies).toEqual([[bunfs, '/cache/superterminal/abc123/superterminal-native.node']]);
    expect(env.env[NAPI_ENV_VAR]).toBe('/cache/superterminal/abc123/superterminal-native.node');
  });

  test('an existing cached copy is reused', () => {
    const target = '/cache/superterminal/abc123/superterminal-native.node';
    const env = fakeEnv({
      execPath: '/opt/st/superterminal',
      env: { XDG_CACHE_HOME: '/cache', SUPERTERMINAL_BUILD_ID: 'abc123' },
      exists: (p) => p === bunfs || p === target,
    });
    expect(locateNative(env)).toMatchObject({ kind: 'copied', path: target });
  });

  test('a failing copy degrades to missing instead of throwing', () => {
    const env = fakeEnv({
      execPath: '/opt/st/superterminal',
      env: { XDG_CACHE_HOME: '/cache' },
      exists: (p) => p === bunfs,
      copy: () => {
        throw new Error('ENOSPC');
      },
    });
    expect(locateNative(env).kind).toBe('missing');
    expect(logsOf(env).join('\n')).toContain('ENOSPC');
  });

  test('cacheDir falls back to ~/.cache', () => {
    expect(cacheDir(fakeEnv({ env: { HOME: '/home/x' } }), 'dev')).toBe(
      '/home/x/.cache/superterminal/dev',
    );
  });
});
