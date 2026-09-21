import { describe, expect, test } from 'bun:test';
import { resolveApiKey } from './key-source.js';
import { deleteStoredKey, last4, readStoredKeys, validateKeyShape, writeStoredKey, type SecretsFs } from './key-store.js';

function memoryFs(initial: Record<string, string> = {}): SecretsFs & { files: Map<string, string> } {
  const files = new Map(Object.entries(initial));
  return {
    files,
    exists: (p) => files.has(p),
    read: (p) => {
      const v = files.get(p);
      if (v === undefined) throw new Error('ENOENT');
      return v;
    },
    write: (p, t) => void files.set(p, t),
    remove: (p) => void files.delete(p),
  };
}

const S = '/s/secrets.json';

describe('key store', () => {
  test('round-trips per-provider keys and never writes anything but the keys', () => {
    const fs = memoryFs();
    writeStoredKey(S, 'typesafe', '  apikey_abc_123  ', fs);
    writeStoredKey(S, 'zen', 'sk-zen12345', fs);
    expect(JSON.parse(fs.files.get(S)!)).toEqual({ ai: { keys: { typesafe: 'apikey_abc_123', zen: 'sk-zen12345' } } });
    expect(readStoredKeys(S, fs)).toEqual({ keys: { typesafe: 'apikey_abc_123', zen: 'sk-zen12345' } });
  });

  test('writing one provider keeps the other', () => {
    const fs = memoryFs({ [S]: '{"ai":{"keys":{"zen":"sk-zen"}}}' });
    writeStoredKey(S, 'typesafe', 'apikey_new', fs);
    expect(readStoredKeys(S, fs).keys).toEqual({ typesafe: 'apikey_new', zen: 'sk-zen' });
    writeStoredKey(S, 'typesafe', 'apikey_newer', fs);
    expect(readStoredKeys(S, fs).keys).toEqual({ typesafe: 'apikey_newer', zen: 'sk-zen' });
  });

  test('the legacy single key reads as zen and is migrated on write', () => {
    const fs = memoryFs({ [S]: '{"ai":{"api_key":"sk-legacy"}}' });
    expect(readStoredKeys(S, fs)).toEqual({ keys: { zen: 'sk-legacy' } });
    writeStoredKey(S, 'typesafe', 'apikey_x', fs);
    expect(JSON.parse(fs.files.get(S)!)).toEqual({ ai: { keys: { typesafe: 'apikey_x', zen: 'sk-legacy' } } });
  });

  test('delete removes one provider, and the file when none remain', () => {
    const fs = memoryFs({ [S]: '{"ai":{"keys":{"typesafe":"apikey_x","zen":"sk-zen"}}}' });
    deleteStoredKey(S, 'typesafe', fs);
    expect(JSON.parse(fs.files.get(S)!)).toEqual({ ai: { keys: { zen: 'sk-zen' } } });
    deleteStoredKey(S, 'typesafe', fs);
    expect(fs.files.has(S)).toBe(true);
    deleteStoredKey(S, 'zen', fs);
    expect(fs.files.has(S)).toBe(false);
    expect(readStoredKeys(S, fs)).toEqual({ keys: {} });
  });

  test('deleting the legacy key removes the file', () => {
    const fs = memoryFs({ [S]: '{"ai":{"api_key":"sk-legacy"}}' });
    deleteStoredKey(S, 'zen', fs);
    expect(fs.files.has(S)).toBe(false);
  });

  test('a corrupt file yields no keys and a warning', () => {
    const fs = memoryFs({ [S]: '{ not json' });
    const r = readStoredKeys(S, fs);
    expect(r.keys).toEqual({});
    expect(r.warning).toContain('not valid JSON');
  });

  test('blank and non-string entries are ignored', () => {
    const fs = memoryFs({ [S]: '{"ai":{"keys":{"typesafe":"  ","zen":42,"other":"x"}}}' });
    expect(readStoredKeys(S, fs).keys).toEqual({});
  });

  test('last4 and the shape check', () => {
    expect(last4('sk-abcdef')).toBe('cdef');
    expect(last4('ab')).toBe('ab');
    expect(validateKeyShape('')).toBe('Paste a key first');
    expect(validateKeyShape('has space here')).toContain('spaces');
    expect(validateKeyShape('short')).toContain('short');
    expect(validateKeyShape('sk-abcdefgh')).toBeNull();
  });
});

describe('resolveApiKey precedence (08 Q15)', () => {
  const opencode = '/home/u/.local/share/opencode/auth.json';
  const ocLogin = '{"opencode":{"type":"api","key":"oc-key"}}';
  const base = { secretsPath: S, home: '/home/u', env: {} as Record<string, string | undefined> };

  describe('explicit zen', () => {
    const setting = 'zen' as const;

    test('config wins over everything', () => {
      const fs = memoryFs({ [S]: '{"ai":{"keys":{"zen":"app-key"}}}' });
      const r = resolveApiKey({ ...base, setting, configKey: 'cfg-key', env: { SUPERTERMINAL_AI_API_KEY: 'env-key' }, fs });
      expect(r.resolved).toEqual({ key: 'cfg-key', source: 'config', provider: 'zen' });
    });

    test('then the app-stored key, then the env, then OpenCode', () => {
      const fs = memoryFs({ [S]: '{"ai":{"api_key":"app-key"}}', [opencode]: ocLogin });
      const env = { SUPERTERMINAL_AI_API_KEY: 'env-key' };
      expect(resolveApiKey({ ...base, setting, env, fs }).resolved).toEqual({ key: 'app-key', source: 'app', provider: 'zen' });
      fs.files.delete(S);
      expect(resolveApiKey({ ...base, setting, env, fs }).resolved).toEqual({ key: 'env-key', source: 'env', provider: 'zen' });
      expect(resolveApiKey({ ...base, setting, fs }).resolved).toEqual({ key: 'oc-key', source: 'opencode', provider: 'zen' });
    });

    test('ignores TYPESAFE_API_KEY and a stored TypeSafe key', () => {
      const fs = memoryFs({ [S]: '{"ai":{"keys":{"typesafe":"apikey_app"}}}', [opencode]: ocLogin });
      const r = resolveApiKey({ ...base, setting, env: { TYPESAFE_API_KEY: 'apikey_env' }, fs });
      expect(r.resolved).toEqual({ key: 'oc-key', source: 'opencode', provider: 'zen' });
    });
  });

  describe('explicit typesafe', () => {
    const setting = 'typesafe' as const;

    test('the provider env var beats the generic one', () => {
      const fs = memoryFs();
      const r = resolveApiKey({ ...base, setting, env: { TYPESAFE_API_KEY: 'apikey_ts', SUPERTERMINAL_AI_API_KEY: 'apikey_gen' }, fs });
      expect(r.resolved).toEqual({ key: 'apikey_ts', source: 'env', provider: 'typesafe' });
    });

    test('never uses the OpenCode login or a stored zen key', () => {
      const fs = memoryFs({ [S]: '{"ai":{"keys":{"zen":"sk-zen"}}}', [opencode]: ocLogin });
      expect(resolveApiKey({ ...base, setting, fs }).resolved).toBeNull();
    });

    test('config api_key belongs to the explicit provider whatever its shape', () => {
      const fs = memoryFs();
      expect(resolveApiKey({ ...base, setting, configKey: 'odd-key', fs }).resolved?.provider).toBe('typesafe');
    });
  });

  describe('auto', () => {
    const setting = 'auto' as const;

    test('a typesafe app key beats a zen OpenCode login', () => {
      const fs = memoryFs({ [S]: '{"ai":{"keys":{"typesafe":"apikey_app"}}}', [opencode]: ocLogin });
      expect(resolveApiKey({ ...base, setting, fs }).resolved).toEqual({ key: 'apikey_app', source: 'app', provider: 'typesafe' });
    });

    test('with both app keys, TypeSafe is preferred', () => {
      const fs = memoryFs({ [S]: '{"ai":{"keys":{"typesafe":"apikey_app","zen":"sk-zen"}}}' });
      expect(resolveApiKey({ ...base, setting, fs }).resolved?.provider).toBe('typesafe');
    });

    test('source precedence beats provider preference', () => {
      const fs = memoryFs({ [S]: '{"ai":{"keys":{"zen":"sk-zen"}}}' });
      const r = resolveApiKey({ ...base, setting, env: { TYPESAFE_API_KEY: 'apikey_env' }, fs });
      expect(r.resolved).toEqual({ key: 'sk-zen', source: 'app', provider: 'zen' });
    });

    test('config api_key is routed by its shape', () => {
      const fs = memoryFs();
      expect(resolveApiKey({ ...base, setting, configKey: 'apikey_c_1', fs }).resolved?.provider).toBe('typesafe');
      expect(resolveApiKey({ ...base, setting, configKey: 'sk-zen', fs }).resolved?.provider).toBe('zen');
    });

    test('SUPERTERMINAL_AI_API_KEY with apikey_ is TypeSafe', () => {
      const fs = memoryFs({ [opencode]: ocLogin });
      const r = resolveApiKey({ ...base, setting, env: { SUPERTERMINAL_AI_API_KEY: 'apikey_gen_1' }, fs });
      expect(r.resolved).toEqual({ key: 'apikey_gen_1', source: 'env', provider: 'typesafe' });
    });

    test('TYPESAFE_API_KEY is checked before the generic var', () => {
      const fs = memoryFs();
      const r = resolveApiKey({ ...base, setting, env: { TYPESAFE_API_KEY: 'apikey_ts', SUPERTERMINAL_AI_API_KEY: 'sk-gen' }, fs });
      expect(r.resolved).toEqual({ key: 'apikey_ts', source: 'env', provider: 'typesafe' });
    });

    test('the OpenCode login is zen', () => {
      const fs = memoryFs({ [opencode]: ocLogin });
      expect(resolveApiKey({ ...base, setting, fs }).resolved).toEqual({ key: 'oc-key', source: 'opencode', provider: 'zen' });
    });
  });

  test('nothing anywhere resolves to null without throwing', () => {
    const fs = memoryFs({ [opencode]: 'garbage' });
    expect(resolveApiKey({ ...base, setting: 'auto', fs }).resolved).toBeNull();
  });

  test('a throwing filesystem resolves to null with a warning', () => {
    const fs: SecretsFs = {
      exists: () => {
        throw new Error('EACCES');
      },
      read: () => '',
      write: () => {},
      remove: () => {},
    };
    const r = resolveApiKey({ ...base, setting: 'auto', fs });
    expect(r.resolved).toBeNull();
    expect(r.warnings.length).toBe(1);
  });

  test('a blank config key does not shadow the others', () => {
    const fs = memoryFs({ [S]: '{"ai":{"keys":{"zen":"app-key"}}}' });
    expect(resolveApiKey({ ...base, setting: 'auto', configKey: '   ', fs }).resolved?.source).toBe('app');
  });
});
