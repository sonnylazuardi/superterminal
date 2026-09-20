/**
 * Jev adapter (docs/plan/08-jev-palette.md §C).
 *
 * Jev is TypeSafe's "System One" model: no text generation. A request carries
 * a `state` (any JSON) and a map of typed `questions`; the answer carries a
 * probability (`noul`), a chosen option with a distribution (`choice`) or a
 * level on an ordered scale (`score`) for each. Default provider is OpenCode
 * Zen, whose `/v1/systemone` passthrough speaks the same shape.
 *
 * `fetch` is injected so nothing here touches the network in tests.
 */

export type JevQuestion =
  | { type: 'noul'; instructions: string; criteria?: Record<string, string> }
  | { type: 'choice'; instructions: string; criteria: Record<string, string> }
  | { type: 'score'; instructions: string; criteria: string[] };

export interface JevNoulAnswer {
  noul: number;
}
export interface JevChoiceAnswer {
  choice: string;
  probabilities: Record<string, number>;
  confidence: number;
}
export interface JevScoreAnswer {
  score: number;
  legend?: string;
  probabilities?: Record<string, number> | number[];
  confidence: number;
}
export type JevAnswer = JevNoulAnswer | JevChoiceAnswer | JevScoreAnswer;

export interface JevResult {
  model: string;
  answers: Record<string, JevAnswer>;
  usage: { input_tokens: number; output_tokens: number };
  /** Wall-clock round trip as measured here. */
  latencyMs: number;
}

/** What went wrong, coarse enough to branch on (08 Q19). */
export type JevErrorKind =
  | 'no_key'
  | 'auth' // 401/403: disable for the session
  | 'model' // provider says the model is unsupported: disable for the session
  | 'rate_limit' // 429 or Zen's free-tier envelope: back off
  | 'bad_request'
  | 'network' // fetch threw, timed out, or was aborted
  | 'server'; // 5xx or an unparsable body

export class JevError extends Error {
  constructor(
    readonly kind: JevErrorKind,
    message: string,
  ) {
    super(message);
    this.name = 'JevError';
  }
}

export interface JevClientOptions {
  endpoint: string;
  model: string;
  /** Returns the current key, or null when none is configured. */
  getKey: () => string | null;
  fetch?: typeof fetch;
  /** Per-request timeout; the palette must never wait long (08 Q19). */
  timeoutMs?: number;
}

export interface JevClient {
  evaluate(
    state: unknown,
    questions: Record<string, JevQuestion>,
    opts?: { signal?: AbortSignal },
  ): Promise<JevResult>;
}

/** Maps a Zen/TypeSafe error body to a kind; exported for the tests. */
export function classifyFailure(status: number, body: unknown): JevError {
  const message = errorMessage(body) ?? `HTTP ${status}`;
  if (status === 401 || status === 403) return new JevError('auth', message);
  if (status === 429 || /rate limit|free tier|too many/i.test(message)) {
    return new JevError('rate_limit', message);
  }
  if (/model .* not supported|ModelError/i.test(message) || errorType(body) === 'ModelError') {
    return new JevError('model', message);
  }
  if (status === 400 || status === 422) return new JevError('bad_request', message);
  return new JevError('server', message);
}

function errorType(body: unknown): string | null {
  if (typeof body !== 'object' || body === null) return null;
  const error = (body as { error?: unknown }).error;
  if (typeof error === 'object' && error !== null) {
    const t = (error as { type?: unknown }).type;
    return typeof t === 'string' ? t : null;
  }
  return null;
}

function errorMessage(body: unknown): string | null {
  if (typeof body !== 'object' || body === null) return null;
  const error = (body as { error?: unknown; message?: unknown }).error;
  if (typeof error === 'string') return error;
  if (typeof error === 'object' && error !== null) {
    const m = (error as { message?: unknown }).message;
    if (typeof m === 'string') return m;
  }
  const m = (body as { message?: unknown }).message;
  return typeof m === 'string' ? m : null;
}

export function createJevClient(options: JevClientOptions): JevClient {
  const doFetch = options.fetch ?? fetch;
  const timeoutMs = options.timeoutMs ?? 2500;

  return {
    async evaluate(state, questions, opts = {}) {
      const key = options.getKey();
      if (!key) throw new JevError('no_key', 'no API key configured');

      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(new Error('timeout')), timeoutMs);
      const onOuterAbort = () => controller.abort(opts.signal?.reason);
      opts.signal?.addEventListener('abort', onOuterAbort, { once: true });
      if (opts.signal?.aborted) onOuterAbort();

      const started = performance.now();
      let response: Response;
      try {
        response = await doFetch(options.endpoint, {
          method: 'POST',
          headers: { Authorization: `Bearer ${key}`, 'Content-Type': 'application/json' },
          body: JSON.stringify({ model: options.model, state, questions }),
          signal: controller.signal,
        });
      } catch (err) {
        throw new JevError('network', err instanceof Error ? err.message : String(err));
      } finally {
        clearTimeout(timer);
        opts.signal?.removeEventListener('abort', onOuterAbort);
      }

      let body: unknown;
      try {
        body = await response.json();
      } catch {
        throw new JevError('server', `unreadable response (HTTP ${response.status})`);
      }
      // Zen reports some failures with HTTP 200 and an error envelope.
      if (!response.ok || errorType(body) !== null || !isResultBody(body)) {
        throw classifyFailure(response.status, body);
      }
      return { ...body, latencyMs: Math.round(performance.now() - started) };
    },
  };
}

function isResultBody(body: unknown): body is Omit<JevResult, 'latencyMs'> {
  if (typeof body !== 'object' || body === null) return false;
  const b = body as { answers?: unknown; usage?: unknown };
  return typeof b.answers === 'object' && b.answers !== null && typeof b.usage === 'object';
}
