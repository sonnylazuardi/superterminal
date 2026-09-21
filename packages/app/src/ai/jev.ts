/**
 * Jev adapter (docs/plan/08-jev-palette.md §C).
 *
 * Jev is TypeSafe's "System One" model: no text generation. A request carries
 * a `state` (any JSON) and a map of typed `questions`; the answer carries a
 * probability (`noul`), a chosen option with a distribution (`choice`) or a
 * level on an ordered scale (`score`) for each. Two providers speak the same
 * body (`providers.ts`): TypeSafe direct (`api.typesafe.ai/v1/systemone`) and
 * OpenCode Zen's `/v1/systemone` passthrough.
 *
 * Their failures do not look alike, so `classifyFailure` knows both:
 *   - TypeSafe: FastAPI-style `{"detail":{"error_type","message"}}` for auth
 *     and usage errors, a plain-string `detail` for validation, 429 / 529
 *     (overloaded) with an optional `Retry-After`, and an
 *     `x-typesafe-request-id` header worth keeping for support.
 *   - Zen: some failures arrive as HTTP 200 with a
 *     `{"type":"error","error":{"type","message"}}` envelope, and the free
 *     tier limit is only recognisable by its text.
 *
 * The target (endpoint, model, key) is read on every call, so switching
 * provider needs no new client.
 *
 * `fetch` is injected so nothing here touches the network in tests.
 */

import { debug } from '../util/debug.js';

const log = debug('st:jev');

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
  /** TypeSafe's `x-typesafe-request-id`, when the provider sent one. */
  requestId?: string;
}

/** What went wrong, coarse enough to branch on (08 Q19). */
export type JevErrorKind =
  | 'no_key'
  | 'auth' // 401/403: disable for the session
  | 'model' // provider says the model is unsupported: disable for the session
  | 'rate_limit' // 429, 529 (overloaded) or Zen's free-tier envelope: back off
  | 'bad_request'
  | 'network' // fetch threw, timed out, or was aborted
  | 'server'; // 5xx or an unparsable body

export class JevError extends Error {
  /** How long the provider asked us to wait (`Retry-After` / `retry-after-ms`). */
  readonly retryAfterMs?: number;
  /** TypeSafe's `x-typesafe-request-id`, when present. */
  readonly requestId?: string;

  constructor(
    readonly kind: JevErrorKind,
    message: string,
    extra: { retryAfterMs?: number | undefined; requestId?: string | undefined } = {},
  ) {
    super(message);
    this.name = 'JevError';
    if (extra.retryAfterMs !== undefined) this.retryAfterMs = extra.retryAfterMs;
    if (extra.requestId !== undefined) this.requestId = extra.requestId;
  }
}

/** Where and with what the next call goes; null when no key is configured. */
export interface JevTarget {
  endpoint: string;
  model: string;
  key: string;
}

export interface JevClientOptions {
  /** Read on every call, so a provider switch takes effect immediately. */
  getTarget: () => JevTarget | null;
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

export const REQUEST_ID_HEADER = 'x-typesafe-request-id';

/**
 * Maps a Zen/TypeSafe error response to a kind; exported for the tests.
 * `headers` supplies the retry hint and the request id when there is one.
 */
export function classifyFailure(status: number, body: unknown, headers?: Headers | null): JevError {
  const message = errorMessage(body) ?? `HTTP ${status}`;
  const type = errorType(body);
  const requestId = headers?.get(REQUEST_ID_HEADER) ?? undefined;
  const make = (kind: JevErrorKind, retryAfterMs?: number) =>
    new JevError(kind, message, { requestId, retryAfterMs });

  if (status === 401 || status === 403 || type === 'authentication_error' || type === 'permission_error') {
    return make('auth');
  }
  if (status === 429 || status === 529 || type === 'rate_limit_error' || /rate limit|free tier|too many|overloaded/i.test(message)) {
    return make('rate_limit', headers ? retryAfter(headers) : undefined);
  }
  if (type === 'ModelError' || /unknown model|model .* not supported|ModelError/i.test(message)) {
    return make('model');
  }
  if (status === 400 || status === 422) return make('bad_request');
  return make('server');
}

/** `retry-after-ms` (milliseconds) or `Retry-After` (seconds or an HTTP date). */
export function retryAfter(headers: Headers, nowMs: number = Date.now()): number | undefined {
  const ms = Number(headers.get('retry-after-ms'));
  if (headers.has('retry-after-ms') && Number.isFinite(ms) && ms >= 0) return Math.round(ms);
  const raw = headers.get('retry-after')?.trim();
  if (!raw) return undefined;
  const seconds = Number(raw);
  if (Number.isFinite(seconds) && seconds >= 0) return Math.round(seconds * 1000);
  const date = Date.parse(raw);
  return Number.isFinite(date) ? Math.max(0, date - nowMs) : undefined;
}

/** Zen's `error.type`, or TypeSafe's `detail.error_type`. */
function errorType(body: unknown): string | null {
  if (typeof body !== 'object' || body === null) return null;
  const error = (body as { error?: unknown }).error;
  if (typeof error === 'object' && error !== null) {
    const t = (error as { type?: unknown }).type;
    if (typeof t === 'string') return t;
  }
  const detail = (body as { detail?: unknown }).detail;
  if (typeof detail === 'object' && detail !== null) {
    const t = (detail as { error_type?: unknown }).error_type;
    if (typeof t === 'string') return t;
  }
  return null;
}

/** Zen's HTTP-200 failure: `{"type":"error","error":{...}}` or any `error` object. */
function isZenErrorEnvelope(body: unknown): boolean {
  if (typeof body !== 'object' || body === null) return false;
  const b = body as { type?: unknown; error?: unknown };
  return b.type === 'error' || (typeof b.error === 'object' && b.error !== null);
}

function errorMessage(body: unknown): string | null {
  if (typeof body !== 'object' || body === null) return null;
  const error = (body as { error?: unknown }).error;
  if (typeof error === 'string') return error;
  if (typeof error === 'object' && error !== null) {
    const m = (error as { message?: unknown }).message;
    if (typeof m === 'string') return m;
  }
  const detail = (body as { detail?: unknown }).detail;
  if (typeof detail === 'string') return detail;
  if (typeof detail === 'object' && detail !== null) {
    const m = (detail as { message?: unknown }).message;
    if (typeof m === 'string') return m;
  }
  if (Array.isArray(detail)) {
    // Pydantic-style validation list: [{ msg, loc }, ...]
    const msgs = detail
      .map((d) => (typeof d === 'object' && d !== null ? (d as { msg?: unknown }).msg : null))
      .filter((m): m is string => typeof m === 'string');
    if (msgs.length > 0) return msgs.join('; ');
  }
  const m = (body as { message?: unknown }).message;
  return typeof m === 'string' ? m : null;
}

export function createJevClient(options: JevClientOptions): JevClient {
  const doFetch = options.fetch ?? fetch;
  const timeoutMs = options.timeoutMs ?? 2500;

  return {
    async evaluate(state, questions, opts = {}) {
      const target = options.getTarget();
      if (!target || !target.key) throw new JevError('no_key', 'no API key configured');
      const { endpoint, model, key } = target;

      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(new Error('timeout')), timeoutMs);
      const onOuterAbort = () => controller.abort(opts.signal?.reason);
      opts.signal?.addEventListener('abort', onOuterAbort, { once: true });
      if (opts.signal?.aborted) onOuterAbort();

      const started = performance.now();
      let response: Response;
      try {
        response = await doFetch(endpoint, {
          method: 'POST',
          headers: { Authorization: `Bearer ${key}`, 'Content-Type': 'application/json' },
          body: JSON.stringify({ model, state, questions }),
          signal: controller.signal,
        });
      } catch (err) {
        throw new JevError('network', err instanceof Error ? err.message : String(err));
      } finally {
        clearTimeout(timer);
        opts.signal?.removeEventListener('abort', onOuterAbort);
      }

      const requestId = response.headers.get(REQUEST_ID_HEADER) ?? undefined;
      if (requestId) log(`${response.status} request id ${requestId}`);
      let body: unknown;
      try {
        body = await response.json();
      } catch {
        // A 429/529 may come without a JSON body; its status still says what happened.
        if (!response.ok) throw classifyFailure(response.status, null, response.headers);
        throw new JevError('server', `unreadable response (HTTP ${response.status})`, { requestId });
      }
      // Zen reports some failures with HTTP 200 and an error envelope.
      if (!response.ok || isZenErrorEnvelope(body) || !isResultBody(body)) {
        throw classifyFailure(response.status, body, response.headers);
      }
      return {
        ...body,
        latencyMs: Math.round(performance.now() - started),
        ...(requestId ? { requestId } : {}),
      };
    },
  };
}

function isResultBody(body: unknown): body is Omit<JevResult, 'latencyMs'> {
  if (typeof body !== 'object' || body === null) return false;
  const b = body as { answers?: unknown; usage?: unknown };
  return typeof b.answers === 'object' && b.answers !== null && typeof b.usage === 'object';
}
