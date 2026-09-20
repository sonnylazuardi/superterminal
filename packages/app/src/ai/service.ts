/**
 * The one object that holds the provider key at runtime (08 Q17a). It owns
 * the Jev client, resolves the key at start-up and after the dialog changes
 * it, and mirrors what the chrome may know into `ui.ai` — never the key.
 *
 * Failure policy (08 Q19): auth/model errors disable Jev for the session,
 * rate limits back off, everything else is logged and the palette stays local.
 */

import type { Config } from '../config/schema.js';
import type { AiKeySource, AiStatus } from '../state/types.js';
import type { WorkspaceStore } from '../state/workspace-store.js';
import { debug } from '../util/debug.js';
import { createJevClient, JevError, type JevClient, type JevQuestion, type JevResult } from './jev.js';
import { resolveApiKey } from './key-source.js';
import { deleteStoredKey, last4, realSecretsFs, secretsPath, writeStoredKey, type SecretsFs } from './key-store.js';

const log = debug('st:ai');

const RATE_LIMIT_BACKOFF_MS = 30_000;

export interface AiServiceOptions {
  config: Config;
  store: WorkspaceStore;
  secretsPath?: string;
  fs?: SecretsFs;
  env?: Record<string, string | undefined>;
  fetch?: typeof fetch;
  now?: () => number;
}

export interface AiService {
  /** Whether a ranking call may be made right now. */
  available(): boolean;
  evaluate(state: unknown, questions: Record<string, JevQuestion>, opts?: { signal?: AbortSignal }): Promise<JevResult>;
  /** Store a key from the dialog and make it current. */
  setKey(key: string): void;
  /** Forget the app-stored key; the source falls through to env/OpenCode. */
  removeKey(): void;
  /** One tiny call; reports latency or the mapped error into `ui.ai`. */
  testConnection(): Promise<{ ok: boolean; latencyMs?: number; error?: string }>;
  snapshot(): AiStatus;
}

export function createAiService(options: AiServiceOptions): AiService {
  const { config, store } = options;
  const fs = options.fs ?? realSecretsFs;
  const path = options.secretsPath ?? secretsPath();
  const now = options.now ?? (() => Date.now());
  const enabled = config.ai.palette;

  let key: string | null = null;
  let source: AiKeySource = 'none';
  let disabledReason: string | null = null;
  let backoffUntil = 0;

  const client: JevClient = createJevClient({
    endpoint: config.ai.endpoint,
    model: config.ai.model,
    getKey: () => key,
    ...(options.fetch ? { fetch: options.fetch } : {}),
  });

  function publish(patch: Partial<AiStatus> = {}): void {
    store.dispatch({
      type: 'ai.setStatus',
      status: {
        enabled,
        source,
        last4: key ? last4(key) : null,
        endpoint: config.ai.endpoint,
        model: config.ai.model,
        status: !key ? 'off' : disabledReason ? 'disabled' : 'ready',
        ...patch,
      },
    });
  }

  function resolve(): void {
    const { resolved, warnings } = resolveApiKey({
      configKey: config.ai.apiKey,
      secretsPath: path,
      fs,
      ...(options.env ? { env: options.env } : {}),
    });
    for (const w of warnings) process.stderr.write(`${w}\n`);
    key = resolved?.key ?? null;
    source = resolved?.source ?? 'none';
    disabledReason = null;
    log(`key from ${source}`);
    publish({ lastError: null });
  }

  function noteFailure(err: unknown): void {
    if (!(err instanceof JevError)) {
      log(`unexpected failure: ${err instanceof Error ? err.message : String(err)}`);
      return;
    }
    log(`${err.kind}: ${err.message}`);
    if (err.kind === 'auth' || err.kind === 'model') {
      disabledReason = err.message;
      publish({ lastError: err.message });
    } else if (err.kind === 'rate_limit') {
      backoffUntil = now() + RATE_LIMIT_BACKOFF_MS;
      publish({ lastError: err.message });
    } else if (err.kind !== 'network') {
      publish({ lastError: err.message });
    }
  }

  // The screen-text opt-in is seeded from config once, not on every publish:
  // the AI Settings dialog may flip it for this session, and a later status
  // update must not quietly put it back.
  store.dispatch({ type: 'ai.setStatus', status: { screenContext: config.ai.screenContext } });
  resolve();

  return {
    available: () => enabled && key !== null && disabledReason === null && now() >= backoffUntil,

    async evaluate(state, questions, opts) {
      try {
        const result = await client.evaluate(state, questions, opts);
        publish({ lastLatencyMs: result.latencyMs, lastError: null });
        return result;
      } catch (err) {
        noteFailure(err);
        throw err;
      }
    },

    setKey(newKey) {
      writeStoredKey(path, newKey, fs);
      resolve();
    },

    removeKey() {
      deleteStoredKey(path, fs);
      resolve();
    },

    async testConnection() {
      if (!key) return { ok: false, error: 'No key found' };
      disabledReason = null;
      backoffUntil = 0;
      try {
        const result = await client.evaluate(
          { text: 'ping' },
          { ok: { type: 'noul', instructions: 'Is the `text` the word ping?' } },
        );
        publish({ lastLatencyMs: result.latencyMs, lastError: null });
        return { ok: true, latencyMs: result.latencyMs };
      } catch (err) {
        noteFailure(err);
        const message = err instanceof Error ? err.message : String(err);
        publish({ lastError: message });
        return { ok: false, error: message };
      }
    },

    snapshot: () => store.getState().ui.ai,
  };
}
