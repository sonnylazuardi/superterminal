/**
 * Where the provider key comes from (08 Q15), and therefore which provider is
 * in use. Source precedence, first match wins:
 *   1. `[ai] api_key` in config.toml
 *   2. the keys stored from the AI Settings dialog (`secrets.json`)
 *   3. the environment: the provider's own var (`$TYPESAFE_API_KEY`), then
 *      `$SUPERTERMINAL_AI_API_KEY`
 *   4. OpenCode's own login: `~/.local/share/opencode/auth.json` → `opencode.key`
 *      (`%USERPROFILE%\.local\share\opencode\auth.json` on Windows, best effort)
 *      — a Zen key, so it only counts for Zen.
 *
 * With an explicit provider only that provider's keys count (config `api_key`
 * is taken to belong to whatever provider is set). Under `auto` the same
 * sources are walked in the same order and the first key found decides the
 * provider: a config or generic-env key by its shape (`detectProvider`), the
 * app store in PROVIDER_ORDER, the env's TypeSafe var before the generic one.
 * Source precedence beats provider preference on purpose: a key the user
 * wrote into config should never lose to one found lying around.
 *
 * Everything is injected so the resolution is a pure function in tests.
 */

import { homedir } from 'node:os';
import { join } from 'node:path';
import type { AiKeySource, ProviderId, ProviderSetting } from '../state/types.js';
import { readStoredKeys, type SecretsFs } from './key-store.js';
import { detectProvider, PROVIDER_ORDER, PROVIDERS } from './providers.js';

export const AI_KEY_ENV = 'SUPERTERMINAL_AI_API_KEY';

export interface ResolveKeyInput {
  setting: ProviderSetting;
  configKey?: string | undefined;
  secretsPath: string;
  env?: Record<string, string | undefined>;
  home?: string;
  fs: SecretsFs;
}

export interface ResolvedKey {
  key: string;
  source: Exclude<AiKeySource, 'none'>;
  provider: ProviderId;
}

export function opencodeAuthPath(home: string): string {
  return join(home, '.local', 'share', 'opencode', 'auth.json');
}

/** `opencode.key` from OpenCode's auth store, or null. Never throws. */
export function readOpencodeKey(path: string, fs: SecretsFs): string | null {
  try {
    if (!fs.exists(path)) return null;
    const raw = JSON.parse(fs.read(path)) as unknown;
    const entry = (raw as Record<string, unknown>)?.['opencode'];
    if (typeof entry !== 'object' || entry === null) return null;
    const key = (entry as { key?: unknown }).key;
    return typeof key === 'string' && key.trim().length > 0 ? key.trim() : null;
  } catch {
    return null;
  }
}

export function resolveApiKey(input: ResolveKeyInput): { resolved: ResolvedKey | null; warnings: string[] } {
  const warnings: string[] = [];
  const env = input.env ?? process.env;
  const explicit: ProviderId | null = input.setting === 'auto' ? null : input.setting;
  const candidates: readonly ProviderId[] = explicit ? [explicit] : PROVIDER_ORDER;
  const found = (key: string, source: ResolvedKey['source'], provider: ProviderId) => ({
    resolved: { key, source, provider },
    warnings,
  });

  try {
    // 1. Config: belongs to the explicit provider, or to whoever its shape says.
    const fromConfig = input.configKey?.trim();
    if (fromConfig) return found(fromConfig, 'config', explicit ?? detectProvider(fromConfig));

    // 2. App store.
    const stored = readStoredKeys(input.secretsPath, input.fs);
    if (stored.warning) warnings.push(stored.warning);
    for (const id of candidates) {
      const key = stored.keys[id];
      if (key) return found(key, 'app', id);
    }

    // 3. Env: provider-specific vars, then the generic one.
    for (const id of candidates) {
      const name = PROVIDERS[id].envKey;
      const key = name ? env[name]?.trim() : undefined;
      if (key) return found(key, 'env', id);
    }
    const generic = env[AI_KEY_ENV]?.trim();
    if (generic) return found(generic, 'env', explicit ?? detectProvider(generic));

    // 4. OpenCode login, for providers that accept it.
    const loginProvider = candidates.find((id) => PROVIDERS[id].opencodeLogin);
    if (loginProvider) {
      const home = input.home ?? env['HOME'] ?? env['USERPROFILE'] ?? homedir();
      const fromOpencode = readOpencodeKey(opencodeAuthPath(home), input.fs);
      if (fromOpencode) return found(fromOpencode, 'opencode', loginProvider);
    }
  } catch (err) {
    warnings.push(`[superterminal] could not resolve the AI key: ${err instanceof Error ? err.message : String(err)}`);
  }
  return { resolved: null, warnings };
}
