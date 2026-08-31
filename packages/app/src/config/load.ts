/**
 * Config loading (05 §1, Q34).
 *
 * `Bun.TOML.parse` at runtime, not `import cfg from './x.toml'`: the static
 * form is resolved and inlined at bundle time, so it cannot read a user file
 * whose path is only known at runtime. A missing file yields defaults; unknown
 * keys and validation errors warn to stderr and are ignored.
 */

import { existsSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { ConfigSchema, DEFAULT_CONFIG, normalizeRawConfig, type Config } from './schema.js';

export interface LoadConfigResult {
  config: Config;
  /** Absolute path that was read, or null when defaults were used. */
  path: string | null;
  warnings: string[];
}

export interface LoadConfigOptions {
  /** Explicit file (`--config`); overrides the XDG lookup. */
  path?: string;
  env?: Record<string, string | undefined>;
  readFile?: (path: string) => string;
  exists?: (path: string) => boolean;
}

/** `$XDG_CONFIG_HOME/superterminal/config.toml`, else `~/.config/…`. */
export function configPath(env: Record<string, string | undefined> = process.env): string {
  if (env['SUPERTERMINAL_CONFIG']) return env['SUPERTERMINAL_CONFIG'];
  const base = env['XDG_CONFIG_HOME'] || join(env['HOME'] || homedir(), '.config');
  return join(base, 'superterminal', 'config.toml');
}

/** Validate an already-parsed TOML object. Never throws. */
export function parseConfig(raw: unknown): { config: Config; warnings: string[] } {
  const warnings: string[] = [];
  const { value, unknownKeys } = normalizeRawConfig(raw);
  for (const key of unknownKeys) {
    warnings.push(`[superterminal] unknown config key ignored: ${key}`);
  }
  const result = ConfigSchema.safeParse(value);
  if (result.success) return { config: result.data, warnings };

  for (const issue of result.error.issues) {
    warnings.push(
      `[superterminal] invalid config at ${issue.path.join('.') || '<root>'}: ${issue.message}`,
    );
  }
  // Salvage what we can: retry per table, dropping the ones that failed.
  const partial: Record<string, unknown> = {};
  if (typeof value === 'object' && value !== null) {
    for (const [table, tableValue] of Object.entries(value as Record<string, unknown>)) {
      const single = ConfigSchema.safeParse({ [table]: tableValue });
      if (single.success) partial[table] = tableValue;
    }
  }
  const fallback = ConfigSchema.safeParse(partial);
  return { config: fallback.success ? fallback.data : DEFAULT_CONFIG, warnings };
}

/** Parse TOML text. Never throws; a syntax error becomes a warning. */
export function parseConfigText(text: string): { config: Config; warnings: string[] } {
  let raw: unknown;
  try {
    raw = Bun.TOML.parse(text);
  } catch (err) {
    return {
      config: DEFAULT_CONFIG,
      warnings: [`[superterminal] config.toml is not valid TOML: ${(err as Error).message}`],
    };
  }
  return parseConfig(raw);
}

export function loadConfig(options: LoadConfigOptions = {}): LoadConfigResult {
  const env = options.env ?? (process.env as Record<string, string | undefined>);
  const exists = options.exists ?? ((p: string) => existsSync(p));
  const read = options.readFile ?? ((p: string) => readFileSync(p, 'utf8'));
  const path = options.path ?? configPath(env);

  if (!exists(path)) {
    return { config: DEFAULT_CONFIG, path: null, warnings: [] };
  }
  let text: string;
  try {
    text = read(path);
  } catch (err) {
    return {
      config: DEFAULT_CONFIG,
      path: null,
      warnings: [`[superterminal] could not read ${path}: ${(err as Error).message}`],
    };
  }
  const { config, warnings } = parseConfigText(text);
  return { config, path, warnings };
}

/** Load and print the warnings; used by the entry point. */
export function loadConfigAndWarn(options: LoadConfigOptions = {}): LoadConfigResult {
  const result = loadConfig(options);
  for (const warning of result.warnings) process.stderr.write(`${warning}\n`);
  return result;
}
