import { describe, expect, test } from 'bun:test';
import { resolveApiKey } from './key-source.js';
import { deleteStoredKey, last4, readStoredKey, validateKeyShape, writeStoredKey, type SecretsFs } from './key-store.js';

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

describe('key store', () => {
  test('round-trips a key and never writes anything but the key', () => {
    const fs = memoryFs();
    writeStoredKey('/s/secrets.json', '  sk-abc12345  ', fs);
    expect(JSON.parse(fs.files.get('/s/secrets.json')!)).toEqual({ ai: { api_key: 'sk-abc12345' } });
    expect(readStoredKey('/s/secrets.json', fs)).toEqual({ key: 'sk-abc12345' });
    deleteStoredKey('/s/secrets.json', fs);
    expect(readStoredKey('/s/secrets.json', fs)).toEqual({ key: null });
  });

  test('a corrupt file yields no key and a warning', () => {
    const fs = memoryFs({ '/s/secrets.json': '{ not json' });
    const r = readStoredKey('/s/secrets.json', fs);
    expect(r.key).toBeNull();
    expect(r.warning).toContain('not valid JSON');
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
  const base = { secretsPath: '/s/secrets.json', home: '/home/u', env: {} as Record<string, string | undefined> };

  test('config wins over everything', () => {
    const fs = memoryFs({ '/s/secrets.json': '{"ai":{"api_key":"app-key"}}' });
    const r = resolveApiKey({ ...base, configKey: 'cfg-key', env: { SUPERTERMINAL_AI_API_KEY: 'env-key' }, fs });
    expect(r.resolved).toEqual({ key: 'cfg-key', source: 'config' });
  });

  test('then the app-stored key, then the env, then OpenCode', () => {
    const fs = memoryFs({
      '/s/secrets.json': '{"ai":{"api_key":"app-key"}}',
      [opencode]: '{"opencode":{"type":"api","key":"oc-key"}}',
    });
    expect(resolveApiKey({ ...base, env: { SUPERTERMINAL_AI_API_KEY: 'env-key' }, fs }).resolved).toEqual({
      key: 'app-key',
      source: 'app',
    });
    fs.files.delete('/s/secrets.json');
    expect(resolveApiKey({ ...base, env: { SUPERTERMINAL_AI_API_KEY: 'env-key' }, fs }).resolved).toEqual({
      key: 'env-key',
      source: 'env',
    });
    expect(resolveApiKey({ ...base, fs }).resolved).toEqual({ key: 'oc-key', source: 'opencode' });
  });

  test('nothing anywhere resolves to null without throwing', () => {
    const fs = memoryFs({ [opencode]: 'garbage' });
    expect(resolveApiKey({ ...base, fs }).resolved).toBeNull();
  });

  test('a blank config key does not shadow the others', () => {
    const fs = memoryFs({ '/s/secrets.json': '{"ai":{"api_key":"app-key"}}' });
    expect(resolveApiKey({ ...base, configKey: '   ', fs }).resolved?.source).toBe('app');
  });
});
