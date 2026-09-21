import { describe, expect, test } from 'bun:test';
import { DEFAULT_CONFIG, type Config } from '../config/schema.js';
import { createWorkspaceStore } from '../state/workspace-store.js';
import type { SecretsFs } from './key-store.js';
import { PROVIDERS } from './providers.js';
import { createAiService, type AiServiceOptions } from './service.js';

const S = '/s/secrets.json';
const OPENCODE = '/home/u/.local/share/opencode/auth.json';

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

const okBody = (model: string) => ({
  model,
  answers: { ok: { noul: 0.99 } },
  usage: { input_tokens: 5, output_tokens: 1 },
});

interface Call {
  url: string;
  model: string;
  auth: string;
}

function setup(opts: {
  files?: Record<string, string>;
  env?: Record<string, string | undefined>;
  ai?: Partial<Config['ai']>;
  providerSetting?: AiServiceOptions['providerSetting'];
  respond?: (call: Call) => Response;
  now?: () => number;
}) {
  const fs = memoryFs(opts.files ?? {});
  const store = createWorkspaceStore();
  const calls: Call[] = [];
  const fetchImpl = ((url: string | URL | Request, init?: RequestInit) => {
    const call: Call = {
      url: String(url),
      model: JSON.parse(String(init?.body)).model,
      auth: (init?.headers as Record<string, string>)['Authorization'] ?? '',
    };
    calls.push(call);
    return Promise.resolve(opts.respond ? opts.respond(call) : Response.json(okBody(call.model === 'jev-1.13.0' ? 'jev-1.13.0' : call.model)));
  }) as typeof fetch;
  const config = { ...DEFAULT_CONFIG, ai: { ...DEFAULT_CONFIG.ai, ...opts.ai } } as Config;
  const service = createAiService({
    config,
    store,
    secretsPath: S,
    fs,
    env: { HOME: '/home/u', ...opts.env },
    fetch: fetchImpl,
    ...(opts.providerSetting ? { providerSetting: opts.providerSetting } : {}),
    ...(opts.now ? { now: opts.now } : {}),
  });
  return { fs, store, calls, service, status: () => store.getState().ui.ai };
}

const bothKeys = {
  [S]: '{"ai":{"keys":{"typesafe":"apikey_app_1234"}}}',
  [OPENCODE]: '{"opencode":{"type":"api","key":"oc-zen-key-9999"}}',
};

describe('AiService provider selection', () => {
  test('auto picks TypeSafe when its key is stored, and publishes its target', () => {
    const { status } = setup({ files: bothKeys });
    expect(status()).toMatchObject({
      provider: 'typesafe',
      providerSetting: 'auto',
      source: 'app',
      last4: '1234',
      endpoint: PROVIDERS.typesafe.endpoint,
      model: 'jev-1.13.0',
      status: 'ready',
    });
  });

  test('setProvider flips endpoint, model and provider in the status and on the wire', async () => {
    const { service, status, calls } = setup({ files: bothKeys });
    service.setProvider('zen');
    expect(status()).toMatchObject({
      provider: 'zen',
      providerSetting: 'zen',
      source: 'opencode',
      last4: '9999',
      endpoint: PROVIDERS.zen.endpoint,
      model: 'jev-1.13',
    });
    await service.evaluate({}, {});
    expect(calls.at(-1)).toEqual({ url: PROVIDERS.zen.endpoint, model: 'jev-1.13', auth: 'Bearer oc-zen-key-9999' });

    service.setProvider('typesafe');
    expect(status()).toMatchObject({ provider: 'typesafe', endpoint: PROVIDERS.typesafe.endpoint, model: 'jev-1.13.0' });
    await service.evaluate({}, {});
    expect(calls.at(-1)).toEqual({ url: PROVIDERS.typesafe.endpoint, model: 'jev-1.13.0', auth: 'Bearer apikey_app_1234' });
  });

  test('the providerSetting option wins over config, config wins over auto', () => {
    expect(setup({ files: bothKeys, ai: { provider: 'zen' } }).status().provider).toBe('zen');
    expect(setup({ files: bothKeys, ai: { provider: 'zen' }, providerSetting: 'typesafe' }).status().provider).toBe('typesafe');
  });

  test('config endpoint/model override the preset', () => {
    const { status } = setup({ files: bothKeys, ai: { model: 'jev-latest' } });
    expect(status()).toMatchObject({ endpoint: PROVIDERS.typesafe.endpoint, model: 'jev-latest' });
  });

  test('an explicit provider without a key is off but shows its target', () => {
    const { status, service } = setup({ files: { [OPENCODE]: bothKeys[OPENCODE]! }, providerSetting: 'typesafe' });
    expect(status()).toMatchObject({ provider: null, status: 'off', endpoint: PROVIDERS.typesafe.endpoint });
    expect(service.available()).toBe(false);
  });
});

describe('AiService keys', () => {
  test('setKey under auto routes an apikey_ key to TypeSafe', () => {
    const { service, fs, status } = setup({ files: { [OPENCODE]: bothKeys[OPENCODE]! } });
    expect(status().provider).toBe('zen');
    service.setKey('apikey_new_abcd');
    expect(JSON.parse(fs.files.get(S)!)).toEqual({ ai: { keys: { typesafe: 'apikey_new_abcd' } } });
    expect(status()).toMatchObject({ provider: 'typesafe', source: 'app', last4: 'abcd', model: 'jev-1.13.0' });
  });

  test('setKey under an explicit setting stores for that provider', () => {
    const { service, fs } = setup({ providerSetting: 'zen' });
    service.setKey('apikey_looks_typesafe');
    expect(JSON.parse(fs.files.get(S)!)).toEqual({ ai: { keys: { zen: 'apikey_looks_typesafe' } } });
  });

  test('removeKey forgets the key in use and falls through to the next source', () => {
    const { service, fs, status } = setup({ files: bothKeys });
    expect(status().provider).toBe('typesafe');
    service.removeKey();
    expect(fs.files.has(S)).toBe(false);
    expect(status()).toMatchObject({ provider: 'zen', source: 'opencode', endpoint: PROVIDERS.zen.endpoint });
  });

  test('removeKey(provider) leaves the other stored key alone', () => {
    const { service, fs } = setup({ files: { [S]: '{"ai":{"keys":{"typesafe":"apikey_a","zen":"sk-zen-1"}}}' } });
    service.removeKey('zen');
    expect(JSON.parse(fs.files.get(S)!)).toEqual({ ai: { keys: { typesafe: 'apikey_a' } } });
  });
});

describe('AiService failures', () => {
  test("TypeSafe's Unknown model disables for the session", async () => {
    const { service, status } = setup({
      files: bothKeys,
      ai: { model: 'jev-1.13' },
      respond: () =>
        Response.json({ detail: { error_type: 'api_usage_error', message: 'Unknown model: jev-1.13' } }, { status: 400 }),
    });
    await expect(service.evaluate({}, {})).rejects.toMatchObject({ kind: 'model' });
    expect(status()).toMatchObject({ status: 'disabled', lastError: 'Unknown model: jev-1.13' });
    expect(service.available()).toBe(false);
  });

  test('a rate limit backs off for Retry-After, else 30 s', async () => {
    let t = 1_000;
    let retry: string | null = '5';
    const { service } = setup({
      files: bothKeys,
      now: () => t,
      respond: () =>
        new Response(JSON.stringify({ detail: 'temporarily overloaded' }), {
          status: 529,
          headers: retry ? { 'Retry-After': retry } : {},
        }),
    });
    await expect(service.evaluate({}, {})).rejects.toMatchObject({ kind: 'rate_limit' });
    expect(service.available()).toBe(false);
    t += 5_000;
    expect(service.available()).toBe(true);

    retry = null;
    await expect(service.evaluate({}, {})).rejects.toMatchObject({ kind: 'rate_limit' });
    t += 29_000;
    expect(service.available()).toBe(false);
    t += 1_000;
    expect(service.available()).toBe(true);
  });

  test('testConnection reports the versioned model that answered', async () => {
    const { service, status } = setup({ files: bothKeys, respond: () => Response.json(okBody('jev-1.13.0')) });
    const r = await service.testConnection();
    expect(r).toMatchObject({ ok: true, model: 'jev-1.13.0' });
    expect(r.latencyMs).toBeGreaterThanOrEqual(0);
    expect(status().lastLatencyMs).not.toBeNull();
  });

  test('testConnection with an auth failure reports the message', async () => {
    const { service, status } = setup({
      files: bothKeys,
      respond: () =>
        Response.json(
          { detail: { error_type: 'authentication_error', message: 'Cannot authenticate with the server.' } },
          { status: 401 },
        ),
    });
    expect(await service.testConnection()).toEqual({ ok: false, error: 'Cannot authenticate with the server.' });
    expect(status().status).toBe('disabled');
  });
});
