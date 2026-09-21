import { describe, expect, test } from 'bun:test';
import { classifyFailure, createJevClient, JevError, retryAfter } from './jev.js';

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
      getTarget: () => ({ endpoint: 'https://example.test/systemone', model: 'jev-1.13', key: 'sk-test' }),
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
      getTarget: () => null,
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
      getTarget: () => ({ endpoint: 'https://example.test', model: 'nope', key: 'k' }),
      fetch: fakeFetch(() => Response.json({ type: 'error', error: { type: 'ModelError', message: 'Model nope is not supported' } })),
    });
    await expect(client.evaluate({}, {})).rejects.toMatchObject({ kind: 'model' });
  });

  test('an aborted request is a network failure', async () => {
    const controller = new AbortController();
    const client = createJevClient({
      getTarget: () => ({ endpoint: 'https://example.test', model: 'm', key: 'k' }),
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
      getTarget: () => ({ endpoint: 'https://example.test', model: 'm', key: 'k' }),
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

describe('TypeSafe direct envelopes', () => {
  const auth = {
    detail: {
      error_type: 'authentication_error',
      message: 'Cannot authenticate with the server. Please check your API key and try again.',
    },
  };
  const unknownModel = { detail: { error_type: 'api_usage_error', message: 'Unknown model: jev-1.13' } };
  const validation = { detail: 'Choice question must have at least one choice: q' };

  test('401 authentication_error is auth, with its message', () => {
    const e = classifyFailure(401, auth);
    expect(e.kind).toBe('auth');
    expect(e.message).toContain('Cannot authenticate');
  });

  test('authentication_error / permission_error are auth whatever the status', () => {
    expect(classifyFailure(400, auth).kind).toBe('auth');
    expect(classifyFailure(403, { detail: { error_type: 'permission_error', message: 'no' } }).kind).toBe('auth');
  });

  test('400 Unknown model is model', () => {
    const e = classifyFailure(400, unknownModel);
    expect(e.kind).toBe('model');
    expect(e.message).toBe('Unknown model: jev-1.13');
  });

  test('a plain-string detail on 400/422 is bad_request', () => {
    expect(classifyFailure(400, validation)).toMatchObject({ kind: 'bad_request', message: validation.detail });
    expect(classifyFailure(422, validation).kind).toBe('bad_request');
    expect(classifyFailure(422, { detail: [{ loc: ['body'], msg: 'field required' }] })).toMatchObject({
      kind: 'bad_request',
      message: 'field required',
    });
  });

  test('429 and 529 are rate_limit, honouring retry hints', () => {
    expect(classifyFailure(429, {}).retryAfterMs).toBeUndefined();
    expect(classifyFailure(429, {}, new Headers({ 'Retry-After': '7' }))).toMatchObject({
      kind: 'rate_limit',
      retryAfterMs: 7000,
    });
    expect(classifyFailure(529, { detail: 'temporarily overloaded' }, new Headers({ 'retry-after-ms': '1500' }))).toMatchObject({
      kind: 'rate_limit',
      retryAfterMs: 1500,
    });
    expect(classifyFailure(529, { detail: 'temporarily overloaded' }).kind).toBe('rate_limit');
  });

  test('retryAfter prefers the millisecond header and reads HTTP dates', () => {
    expect(retryAfter(new Headers({ 'retry-after-ms': '250', 'retry-after': '9' }))).toBe(250);
    expect(retryAfter(new Headers({ 'retry-after': 'Mon, 21 Sep 2026 00:00:10 GMT' }), Date.parse('Mon, 21 Sep 2026 00:00:00 GMT'))).toBe(10_000);
    expect(retryAfter(new Headers({ 'retry-after': 'soon' }))).toBeUndefined();
    expect(retryAfter(new Headers())).toBeUndefined();
  });

  test('the request id rides on results and errors', async () => {
    const headers = { 'x-typesafe-request-id': 'req_123' };
    const good = createJevClient({
      getTarget: () => ({ endpoint: 'https://api.test', model: 'jev-1.13.0', key: 'apikey_x' }),
      fetch: fakeFetch(() => Response.json({ ...ok, model: 'jev-1.13.0' }, { headers })),
    });
    expect(await good.evaluate({}, {})).toMatchObject({ model: 'jev-1.13.0', requestId: 'req_123' });

    const bad = createJevClient({
      getTarget: () => ({ endpoint: 'https://api.test', model: 'jev-1.13', key: 'apikey_x' }),
      fetch: fakeFetch(() => Response.json(unknownModel, { status: 400, headers })),
    });
    await expect(bad.evaluate({}, {})).rejects.toMatchObject({ kind: 'model', requestId: 'req_123' });
  });

  test('a 429 without a JSON body is still rate_limit with its Retry-After', async () => {
    const client = createJevClient({
      getTarget: () => ({ endpoint: 'https://api.test', model: 'm', key: 'k' }),
      fetch: fakeFetch(() => new Response('slow down', { status: 429, headers: { 'Retry-After': '2' } })),
    });
    await expect(client.evaluate({}, {})).rejects.toMatchObject({ kind: 'rate_limit', retryAfterMs: 2000 });
  });

  test('the target is read on every call', async () => {
    let target = { endpoint: 'https://a.test', model: 'm1', key: 'k1' };
    const seen: string[] = [];
    const client = createJevClient({
      getTarget: () => target,
      fetch: fakeFetch((url, init) => {
        seen.push(`${url} ${JSON.parse(String(init.body)).model}`);
        return Response.json(ok);
      }),
    });
    await client.evaluate({}, {});
    target = { endpoint: 'https://b.test', model: 'm2', key: 'k2' };
    await client.evaluate({}, {});
    expect(seen).toEqual(['https://a.test m1', 'https://b.test m2']);
  });
});
