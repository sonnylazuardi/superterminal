import { describe, expect, test } from 'bun:test';
import { classifyFailure, createJevClient, JevError } from './jev.js';

function fakeFetch(handler: (url: string, init: RequestInit) => Response | Promise<Response>): typeof fetch {
  return ((url: string | URL | Request, init?: RequestInit) => Promise.resolve(handler(String(url), init ?? {}))) as typeof fetch;
}

const ok = {
  model: 'jev-1.13',
  answers: { pick: { choice: 'cmd:tab.close', probabilities: { 'cmd:tab.close': 0.9 }, confidence: 0.9 } },
  usage: { input_tokens: 10, output_tokens: 1 },
};

describe('createJevClient', () => {
  test('posts the System One shape with the bearer key', async () => {
    let seen: { url: string; init: RequestInit } | null = null;
    const client = createJevClient({
      endpoint: 'https://example.test/systemone',
      model: 'jev-1.13',
      getKey: () => 'sk-test',
      fetch: fakeFetch((url, init) => {
        seen = { url, init };
        return Response.json(ok);
      }),
    });
    const result = await client.evaluate({ query: 'x' }, { q: { type: 'noul', instructions: 'i' } });
    expect(seen!.url).toBe('https://example.test/systemone');
    expect((seen!.init.headers as Record<string, string>)['Authorization']).toBe('Bearer sk-test');
    expect(JSON.parse(String(seen!.init.body))).toEqual({
      model: 'jev-1.13',
      state: { query: 'x' },
      questions: { q: { type: 'noul', instructions: 'i' } },
    });
    expect(result.answers['pick']).toMatchObject({ choice: 'cmd:tab.close' });
    expect(result.latencyMs).toBeGreaterThanOrEqual(0);
  });

  test('no key: fails before touching the network', async () => {
    let calls = 0;
    const client = createJevClient({
      endpoint: 'https://example.test',
      model: 'm',
      getKey: () => null,
      fetch: fakeFetch(() => {
        calls++;
        return Response.json(ok);
      }),
    });
    await expect(client.evaluate({}, {})).rejects.toMatchObject({ kind: 'no_key' });
    expect(calls).toBe(0);
  });

  test("Zen's HTTP 200 error envelope is a failure", async () => {
    const client = createJevClient({
      endpoint: 'https://example.test',
      model: 'nope',
      getKey: () => 'k',
      fetch: fakeFetch(() => Response.json({ type: 'error', error: { type: 'ModelError', message: 'Model nope is not supported' } })),
    });
    await expect(client.evaluate({}, {})).rejects.toMatchObject({ kind: 'model' });
  });

  test('an aborted request is a network failure', async () => {
    const controller = new AbortController();
    const client = createJevClient({
      endpoint: 'https://example.test',
      model: 'm',
      getKey: () => 'k',
      fetch: fakeFetch((_url, init) =>
        new Promise((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => reject(new Error('aborted')));
        }),
      ),
    });
    const pending = client.evaluate({}, {}, { signal: controller.signal });
    controller.abort();
    await expect(pending).rejects.toMatchObject({ kind: 'network' });
  });

  test('a timeout is a network failure', async () => {
    const client = createJevClient({
      endpoint: 'https://example.test',
      model: 'm',
      getKey: () => 'k',
      timeoutMs: 5,
      fetch: fakeFetch((_url, init) =>
        new Promise((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => reject(new Error('timeout')));
        }),
      ),
    });
    await expect(client.evaluate({}, {})).rejects.toMatchObject({ kind: 'network' });
  });
});

describe('classifyFailure', () => {
  test('maps status codes and envelopes', () => {
    expect(classifyFailure(401, {}).kind).toBe('auth');
    expect(classifyFailure(403, {}).kind).toBe('auth');
    expect(classifyFailure(429, {}).kind).toBe('rate_limit');
    expect(classifyFailure(200, { error: { message: 'free tier limit reached' } }).kind).toBe('rate_limit');
    expect(classifyFailure(400, { error: 'bad' }).kind).toBe('bad_request');
    expect(classifyFailure(500, 'nonsense').kind).toBe('server');
    expect(classifyFailure(500, {})).toBeInstanceOf(JevError);
  });
});
