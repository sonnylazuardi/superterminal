/**
 * The app-stored provider key (08 Q15/Q17a): `secrets.json` beside the Client
 * State file. A key typed into the AI Settings dialog is neither Config
 * (hand-written, never written by the program) nor Client State (remembered
 * without being declared), so it gets its own file. User-only permissions on
 * Unix; on Windows the per-user profile ACL is the protection.
 *
 * The filesystem is injected so the tests never touch the real one.
 */

import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { stateDir, type PathEnv } from '../server/paths.js';

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

/** The key on disk, or null (missing, unreadable or malformed file). */
export function readStoredKey(path: string, fs: SecretsFs = realSecretsFs): { key: string | null; warning?: string } {
  if (!fs.exists(path)) return { key: null };
  let text: string;
  try {
    text = fs.read(path);
  } catch (err) {
    return { key: null, warning: `[superterminal] could not read ${path}: ${(err as Error).message}` };
  }
  try {
    const raw = JSON.parse(text) as unknown;
    const key = (raw as { ai?: { api_key?: unknown } })?.ai?.api_key;
    if (typeof key === 'string' && key.trim().length > 0) return { key: key.trim() };
    return { key: null };
  } catch {
    return { key: null, warning: `[superterminal] ${path} is not valid JSON; ignoring it` };
  }
}

export function writeStoredKey(path: string, key: string, fs: SecretsFs = realSecretsFs): void {
  fs.write(path, `${JSON.stringify({ ai: { api_key: key.trim() } }, null, 2)}\n`);
}

export function deleteStoredKey(path: string, fs: SecretsFs = realSecretsFs): void {
  fs.remove(path);
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
