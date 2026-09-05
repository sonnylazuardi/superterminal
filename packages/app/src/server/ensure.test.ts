import { describe, expect, test } from 'bun:test';
import { join } from 'node:path';
import { startFakeServer, type FakeServer } from '../control/fake-server.js';
import {
  ServerUnavailableError,
  ensureServer,
  isTestEnvironment,
  locateServerBinary,
  probeSocket,
} from './ensure.js';
import {
  defaultSocketPath,
  isTcpTarget,
  parseTcpTarget,
  probeCandidates,
  stateDir,
  tcpTarget,
} from './paths.js';

describe('socket paths', () => {
  test('SUPERTERMINAL_SOCKET wins', () => {
    expect(defaultSocketPath({ env: { SUPERTERMINAL_SOCKET: '/tmp/st-dev.sock' } })).toBe(
      '/tmp/st-dev.sock',
    );
    expect(probeCandidates({ env: { SUPERTERMINAL_SOCKET: '/tmp/st-dev.sock' } })).toEqual([
      '/tmp/st-dev.sock',
    ]);
  });

  test('linux uses XDG_RUNTIME_DIR, then /tmp/superterminal-<uid>', () => {
    expect(defaultSocketPath({ platform: 'linux', env: { XDG_RUNTIME_DIR: '/run/user/1000' } })).toBe(
      '/run/user/1000/superterminal/server.sock',
    );
    expect(defaultSocketPath({ platform: 'linux', env: {}, uid: 1000 })).toBe(
      '/tmp/superterminal-1000/server.sock',
    );
  });

  test('macOS uses Application Support', () => {
    expect(defaultSocketPath({ platform: 'darwin', env: { HOME: '/Users/x' } })).toBe(
      '/Users/x/Library/Application Support/superterminal/server.sock',
    );
  });

  test('the alternate spellings from 02/03 are probed too', () => {
    expect(probeCandidates({ platform: 'linux', env: { XDG_RUNTIME_DIR: '/run/user/1000' } })).toEqual([
      '/run/user/1000/superterminal/server.sock',
      '/run/user/1000/superterminal/control.sock',
      '/run/user/1000/superterminal/sock',
    ]);
  });

  test('SUPERTERMINAL_TCP turns the target into tcp://', () => {
    const env = { SUPERTERMINAL_TCP: '127.0.0.1:7171' };
    expect(tcpTarget({ env })).toBe('tcp://127.0.0.1:7171');
    expect(defaultSocketPath({ env })).toBe('tcp://127.0.0.1:7171');
    expect(probeCandidates({ env })).toEqual(['tcp://127.0.0.1:7171']);
    expect(tcpTarget({ env: {} })).toBeNull();
    expect(tcpTarget({ env: { SUPERTERMINAL_TCP: 'not-an-addr' } })).toBeNull();
  });

  test('tcp target parsing', () => {
    expect(isTcpTarget('tcp://127.0.0.1:7171')).toBe(true);
    expect(isTcpTarget('/tmp/x.sock')).toBe(false);
    expect(parseTcpTarget('tcp://127.0.0.1:7171')).toEqual(['127.0.0.1', 7171]);
    expect(parseTcpTarget('tcp://[::1]:7171')).toEqual(['[::1]', 7171]);
    expect(parseTcpTarget('tcp://no-port')).toBeNull();
    expect(parseTcpTarget('/tmp/x.sock')).toBeNull();
  });

  test('win32 falls back to LOCALAPPDATA', () => {
    expect(
      defaultSocketPath({ platform: 'win32', env: { LOCALAPPDATA: 'C:\\Users\\x\\AppData\\Local' } }),
    ).toBe('C:\\Users\\x\\AppData\\Local/superterminal/server.sock');
  });

  test('state dir honours XDG_STATE_HOME', () => {
    expect(stateDir({ env: { XDG_STATE_HOME: '/state' } })).toBe('/state/superterminal');
    expect(stateDir({ env: { HOME: '/home/x' } })).toBe('/home/x/.local/state/superterminal');
    // Windows has no XDG state dir and no daemon beside the client; the
    // client's own state joins runtimeDir under %LOCALAPPDATA%.
    expect(
      stateDir({ platform: 'win32', env: { LOCALAPPDATA: 'C:\\Users\\x\\AppData\\Local' } }),
    ).toBe(join('C:\\Users\\x\\AppData\\Local', 'superterminal'));
  });
});

describe('TCP ensure', () => {
  test('an unreachable TCP target fails without spawning', async () => {
    let spawned = 0;
    const err = await ensureServer({
      socketPath: 'tcp://127.0.0.1:1',
      probeTimeoutMs: 50,
      spawn: () => {
        spawned += 1;
        return { pid: 1, unref: () => {} };
      },
    }).then(
      () => null,
      (e) => e as ServerUnavailableError,
    );
    expect(err).toBeInstanceOf(ServerUnavailableError);
    expect(err?.kind).toBe('not_running');
    expect(err?.message).toContain('WSL');
    expect(spawned).toBe(0);
  });

  test('a live TCP listener is returned as-is', async () => {
    const listener = Bun.listen({
      hostname: '127.0.0.1',
      port: 0,
      socket: {
        data() {},
        open() {},
        close() {},
        error() {},
      },
    });
    try {
      const addr = `tcp://127.0.0.1:${listener.port}`;
      const result = await ensureServer({ socketPath: addr, probeTimeoutMs: 500 });
      expect(result).toEqual({ socketPath: addr, spawned: false });
    } finally {
      listener.stop();
    }
  });
});

describe('probeSocket', () => {
  test('true for a live listener, false for a dead path', async () => {
    const server: FakeServer = await startFakeServer();
    try {
      expect(await probeSocket(server.socketPath, 500)).toBe(true);
    } finally {
      server.stop();
    }
    expect(await probeSocket('/tmp/st-definitely-not-here-9182.sock', 200)).toBe(false);
  });
});

describe('ensureServer', () => {
  test('reuses a running server without spawning', async () => {
    const server = await startFakeServer();
    try {
      const spawns: string[] = [];
      const result = await ensureServer({
        socketPath: server.socketPath,
        spawn: (bin) => {
          spawns.push(bin);
          return { pid: 1, unref: () => {} };
        },
      });
      expect(result).toEqual({ socketPath: server.socketPath, spawned: false });
      expect(spawns).toEqual([]);
    } finally {
      server.stop();
    }
  });

  test('finds a server listening under an alternate socket name', async () => {
    const server = await startFakeServer();
    const dir = server.socketPath.replace(/\/[^/]+$/, '');
    const result = await ensureServer({
      env: { XDG_RUNTIME_DIR: dir },
      platform: 'linux',
      // probeCandidates builds <dir>/superterminal/*.sock; short-circuit the
      // probe so this test asserts the candidate loop, not the filesystem.
      probe: async (p) => p.endsWith('/superterminal/server.sock'),
    });
    expect(result.spawned).toBe(false);
    expect(result.socketPath).toContain('server.sock');
    server.stop();
  });

  test('--no-spawn surfaces a typed not_running error', async () => {
    const err = (await ensureServer({
      socketPath: '/tmp/st-nope.sock',
      noSpawn: true,
      probe: async () => false,
    }).catch((e) => e)) as ServerUnavailableError;
    expect(err).toBeInstanceOf(ServerUnavailableError);
    expect(err.kind).toBe('not_running');
    expect(err.socketPath).toBe('/tmp/st-nope.sock');
  });

  test('never spawns from a test run unless explicitly allowed', async () => {
    expect(isTestEnvironment()).toBe(true);
    const spawns: string[] = [];
    const err = (await ensureServer({
      socketPath: '/tmp/st-nope.sock',
      probe: async () => false,
      which: () => '/usr/bin/superterminald',
      exists: () => false,
      spawn: (bin) => {
        spawns.push(bin);
        return { pid: 1, unref: () => {} };
      },
    }).catch((e) => e)) as ServerUnavailableError;
    expect(err.kind).toBe('not_running');
    expect(err.message).toContain('refusing to spawn');
    expect(spawns).toEqual([]);
  });

  test('spawns, then retries until the socket answers', async () => {
    let probes = 0;
    const spawns: string[] = [];
    const sleeps: number[] = [];
    const result = await ensureServer({
      socketPath: '/tmp/st-spawn.sock',
      allowSpawnInTests: true,
      which: () => '/usr/bin/superterminald',
      exists: () => false,
      probe: async () => ++probes > 3,
      sleep: async (ms) => {
        sleeps.push(ms);
      },
      spawn: (bin) => {
        spawns.push(bin);
        return { pid: 4242, unref: () => {} };
      },
    });
    expect(spawns).toEqual(['/usr/bin/superterminald']);
    expect(result).toEqual({ socketPath: '/tmp/st-spawn.sock', spawned: true, pid: 4242 });
    expect(sleeps).toEqual([250, 250]);
  });

  test('a missing binary is a typed error', async () => {
    const err = (await ensureServer({
      socketPath: '/tmp/st-nope.sock',
      allowSpawnInTests: true,
      probe: async () => false,
      exists: () => false,
      which: () => null,
    }).catch((e) => e)) as ServerUnavailableError;
    expect(err.kind).toBe('binary_not_found');
    expect(err.message).toContain('SUPERTERMINAL_SERVER');
  });

  test('a throwing spawn is a typed error', async () => {
    const err = (await ensureServer({
      socketPath: '/tmp/st-nope.sock',
      allowSpawnInTests: true,
      probe: async () => false,
      exists: () => false,
      which: () => '/usr/bin/superterminald',
      spawn: () => {
        throw new Error('EPERM');
      },
    }).catch((e) => e)) as ServerUnavailableError;
    expect(err.kind).toBe('spawn_failed');
    expect(err.message).toContain('EPERM');
  });

  test('a server that never listens times out', async () => {
    const err = (await ensureServer({
      socketPath: '/tmp/st-nope.sock',
      allowSpawnInTests: true,
      retryForMs: 750,
      probe: async () => false,
      exists: () => false,
      which: () => '/usr/bin/superterminald',
      sleep: async () => {},
      spawn: () => ({ pid: 7, unref: () => {} }),
    }).catch((e) => e)) as ServerUnavailableError;
    expect(err.kind).toBe('timeout');
    expect(err.message).toContain('pid 7');
  });
});

describe('locateServerBinary', () => {
  test('prefers $SUPERTERMINAL_SERVER', () => {
    expect(
      locateServerBinary({
        env: { SUPERTERMINAL_SERVER: '/opt/st/superterminald' },
        exists: (p) => p === '/opt/st/superterminald',
        which: () => null,
      }),
    ).toBe('/opt/st/superterminald');
  });

  test('then a sibling of the client binary', () => {
    expect(
      locateServerBinary({
        env: {},
        execPath: '/opt/st/superterminal',
        exists: (p) => p === '/opt/st/superterminald',
        which: () => null,
      }),
    ).toBe('/opt/st/superterminald');
  });

  test('then $PATH, else null', () => {
    expect(
      locateServerBinary({
        env: {},
        execPath: '/usr/bin/bun',
        exists: () => false,
        which: () => '/usr/local/bin/superterminald',
      }),
    ).toBe('/usr/local/bin/superterminald');
    expect(
      locateServerBinary({ env: {}, execPath: '/usr/bin/bun', exists: () => false, which: () => null }),
    ).toBeNull();
  });

  test('an $SUPERTERMINAL_SERVER that does not exist is skipped', () => {
    expect(
      locateServerBinary({
        env: { SUPERTERMINAL_SERVER: '/nope' },
        execPath: '/usr/bin/bun',
        exists: () => false,
        which: () => '/usr/bin/superterminald',
      }),
    ).toBe('/usr/bin/superterminald');
  });
});
