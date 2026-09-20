/**
 * Where the provider key comes from (08 Q15), first match wins:
 *   1. `[ai] api_key` in config.toml
 *   2. the key stored from the AI Settings dialog (`secrets.json`)
 *   3. `$SUPERTERMINAL_AI_API_KEY`
 *   4. OpenCode's own login: `~/.local/share/opencode/auth.json` → `opencode.key`
 *      (`%USERPROFILE%\.local\share\opencode\auth.json` on Windows, best effort)
 *
 * Everything is injected so the resolution is a pure function in tests.
 */

import { homedir } from 'node:os';
import { join } from 'node:path';
import type { AiKeySource } from '../state/types.js';
import { readStoredKey, type SecretsFs } from './key-store.js';

export const AI_KEY_ENV = 'SUPERTERMINAL_AI_API_KEY';

export interface ResolveKeyInput {
  configKey?: string | undefined;
  secretsPath: string;
  env?: Record<string, string | undefined>;
  home?: string;
  fs: SecretsFs;
}

export interface ResolvedKey {
  key: string;
  source: Exclude<AiKeySource, 'none'>;
}

export function opencodeAuthPath(home: string): string {
  return join(home, '.local', 'share', 'opencode', 'auth.json');
}

/** `opencode.key` from OpenCode's auth store, or null. Never throws. */
export function readOpencodeKey(path: string, fs: SecretsFs): string | null {
  if (!fs.exists(path)) return null;
  try {
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

  const fromConfig = input.configKey?.trim();
  if (fromConfig) return { resolved: { key: fromConfig, source: 'config' }, warnings };

  const stored = readStoredKey(input.secretsPath, input.fs);
  if (stored.warning) warnings.push(stored.warning);
  if (stored.key) return { resolved: { key: stored.key, source: 'app' }, warnings };

  const fromEnv = env[AI_KEY_ENV]?.trim();
  if (fromEnv) return { resolved: { key: fromEnv, source: 'env' }, warnings };

  const home = input.home ?? env['HOME'] ?? env['USERPROFILE'] ?? homedir();
  const fromOpencode = readOpencodeKey(opencodeAuthPath(home), input.fs);
  if (fromOpencode) return { resolved: { key: fromOpencode, source: 'opencode' }, warnings };

  return { resolved: null, warnings };
}
