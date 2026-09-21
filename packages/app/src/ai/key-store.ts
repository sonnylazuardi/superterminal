/**
 * The app-stored provider key (08 Q15/Q17a): `secrets.json` beside the Client
 * State file. A key typed into the AI Settings dialog is neither Config
 * (hand-written, never written by the program) nor Client State (remembered
 * without being declared), so it gets its own file. User-only permissions on
 * Unix; on Windows the per-user profile ACL is the protection.
 *
 * One key per provider (`{"ai":{"keys":{"typesafe","zen"}}}`), so a user can
 * hold both and flip the provider setting without re-pasting. The legacy
 * single-key shape is read as Zen's and rewritten on the next write.
 *
 * The filesystem is injected so the tests never touch the real one.
 */

import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { stateDir, type PathEnv } from '../server/paths.js';
import type { ProviderId } from '../state/types.js';
import { PROVIDER_ORDER } from './providers.js';

export const SECRETS_FILENAME = 'secrets.json';

export interface SecretsFs {
  exists(path: string): boolean;
  read(path: string): string;
  write(path: string, text: string): void;
  remove(path: string): void;
}

export const realSecretsFs: SecretsFs = {
  exists: (path) => existsSync(path),
  read: (path) => readFileSync(path, 'utf8'),
  write(path, text) {
    mkdirSync(dirname(path), { recursive: true });
    const tmp = `${path}.${process.pid}.tmp`;
    writeFileSync(tmp, text, { encoding: 'utf8', mode: 0o600 });
    try {
      chmodSync(tmp, 0o600);
    } catch {
      /* Windows: no POSIX mode; the profile directory is per-user already. */
    }
    renameSync(tmp, path);
  },
  remove(path) {
    try {
      unlinkSync(path);
    } catch (err) {
      if ((err as NodeJS.ErrnoException).code !== 'ENOENT') throw err;
    }
  },
};

export function secretsPath(input: PathEnv = {}): string {
  return join(stateDir(input), SECRETS_FILENAME);
}

export type StoredKeys = Partial<Record<ProviderId, string>>;

/**
 * Parses the file into per-provider keys. The pre-provider shape
 * `{"ai":{"api_key"}}` was only ever used with Zen, so it reads as the zen
 * key; a `keys.zen` entry wins over it if both somehow exist.
 */
function parseKeys(raw: unknown): StoredKeys {
  const keys: StoredKeys = {};
  const ai = (raw as { ai?: unknown } | null)?.ai;
  if (typeof ai !== 'object' || ai === null) return keys;
  const legacy = (ai as { api_key?: unknown }).api_key;
  if (typeof legacy === 'string' && legacy.trim().length > 0) keys.zen = legacy.trim();
  const map = (ai as { keys?: unknown }).keys;
  if (typeof map === 'object' && map !== null) {
    for (const id of PROVIDER_ORDER) {
      const v = (map as Record<string, unknown>)[id];
      if (typeof v === 'string' && v.trim().length > 0) keys[id] = v.trim();
    }
  }
  return keys;
}

/** Every stored key (missing, unreadable or malformed file: none, maybe a warning). */
export function readStoredKeys(path: string, fs: SecretsFs = realSecretsFs): { keys: StoredKeys; warning?: string } {
  if (!fs.exists(path)) return { keys: {} };
  let text: string;
  try {
    text = fs.read(path);
  } catch (err) {
    return { keys: {}, warning: `[superterminal] could not read ${path}: ${(err as Error).message}` };
  }
  try {
    return { keys: parseKeys(JSON.parse(text) as unknown) };
  } catch {
    return { keys: {}, warning: `[superterminal] ${path} is not valid JSON; ignoring it` };
  }
}

/** Writes the new shape only; a legacy `api_key` is carried over as `keys.zen`. */
function writeKeys(path: string, keys: StoredKeys, fs: SecretsFs): void {
  const ordered: StoredKeys = {};
  for (const id of PROVIDER_ORDER) {
    const k = keys[id];
    if (k) ordered[id] = k;
  }
  if (Object.keys(ordered).length === 0) {
    fs.remove(path);
    return;
  }
  fs.write(path, `${JSON.stringify({ ai: { keys: ordered } }, null, 2)}\n`);
}

/** Stores one provider's key, keeping the other's. */
export function writeStoredKey(path: string, provider: ProviderId, key: string, fs: SecretsFs = realSecretsFs): void {
  const { keys } = readStoredKeys(path, fs);
  writeKeys(path, { ...keys, [provider]: key.trim() }, fs);
}

/** Forgets one provider's key; the file goes away when none remain. */
export function deleteStoredKey(path: string, provider: ProviderId, fs: SecretsFs = realSecretsFs): void {
  const { keys } = readStoredKeys(path, fs);
  delete keys[provider];
  writeKeys(path, keys, fs);
}

/** What the chrome may show of a key: its last four characters (08 Q15). */
export function last4(key: string): string {
  const trimmed = key.trim();
  return trimmed.length <= 4 ? trimmed : trimmed.slice(-4);
}

/** Shape check for the dialog: something was typed and it has no whitespace inside. */
export function validateKeyShape(key: string): string | null {
  const trimmed = key.trim();
  if (trimmed.length === 0) return 'Paste a key first';
  if (/\s/.test(trimmed)) return 'A key has no spaces in it';
  if (trimmed.length < 8) return 'That looks too short for a key';
  return null;
}
