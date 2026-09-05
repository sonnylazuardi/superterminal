/**
 * `config.toml` schema (05 §1, Q34, Q46).
 *
 * The same file is read by the Rust `st-config` crate (server + CLI) and by
 * this app; Q46 says both are validated by a shared fixture set. That fixture
 * set does not exist yet and `docs/config-example.toml` was not present when
 * this was written, so the key spellings here are a judgement call:
 *
 *   - **snake_case is canonical** — it is what the docs use whenever they quote
 *     a real key (`server.idle_exit_minutes` in 03, `scrollback_lines` in 02,
 *     `bold_is_bright` in Q48, `[shell].program`/`[shell].login` in 03), and it
 *     is what serde produces for the Rust structs by default.
 *   - camelCase spellings (`lineHeight`, `verticalTabs`, …) are **also
 *     accepted**, because 05 §1's illustrative zod snippet uses them.
 *
 * DIVERGENCE RISK: if `st-config` lands with only one spelling, delete the
 * other here and update the fixtures. Nothing else in the app depends on the
 * wire spelling — everything downstream reads the camelCase `Config` type.
 */

import { z } from 'zod';

export const WindowBackgroundSchema = z.enum(['auto', 'blurred', 'transparent', 'opaque']);
export type WindowBackground = z.infer<typeof WindowBackgroundSchema>;

export const CursorStyleSchema = z.enum(['block', 'beam', 'underline']);
export const ScrollbarSchema = z.enum(['auto', 'always', 'never']);

export const FontSchema = z
  .object({
    family: z.string().optional(),
    size: z.number().positive().default(13),
    lineHeight: z.number().positive().default(1.2),
  })
  .prefault({});

export const WindowSchema = z
  .object({
    background: WindowBackgroundSchema.default('auto'),
    verticalTabs: z.boolean().default(true),
    width: z.number().positive().optional(),
    height: z.number().positive().optional(),
    /**
     * Remember the last window size and tab layout across launches (Client
     * State, ADR 0008). When true, what was remembered wins over `width`,
     * `height` and `verticalTabs` above, which then only seed the first run.
     */
    remember: z.boolean().default(true),
  })
  .prefault({});

/** Server-owned, but parsed so an unknown-key warning is not raised for it. */
export const ShellSchema = z
  .object({
    program: z.string().optional(),
    args: z.array(z.string()).default([]),
    login: z.boolean().optional(),
    env: z.record(z.string(), z.string()).default({}),
  })
  .prefault({});

export const TerminalSchema = z
  .object({
    scrollbackLines: z.number().int().min(0).max(100_000).default(10_000),
    boldIsBright: z.boolean().default(false),
    cursorStyle: CursorStyleSchema.default('block'),
    cursorBlink: z.boolean().default(true),
    scrollbar: ScrollbarSchema.default('auto'),
  })
  .prefault({});

/** Palette overrides; 04 §10 defines the keys (`ansi0`…`ansi15`, `fg`, `bg`…). */
export const ThemeSchema = z.record(z.string(), z.string()).default({});

/** commandId -> `"mod+shift+t"`. */
export const KeybindingsSchema = z.record(z.string(), z.string()).default({});

export const ConfigSchema = z.object({
  font: FontSchema,
  window: WindowSchema,
  shell: ShellSchema,
  terminal: TerminalSchema,
  theme: ThemeSchema,
  keybindings: KeybindingsSchema,
});

export type Config = z.infer<typeof ConfigSchema>;

export const DEFAULT_CONFIG: Config = ConfigSchema.parse({});

/** Table name -> { wire spelling -> canonical camelCase field }. */
const KEY_ALIASES: Record<string, Record<string, string>> = {
  font: { line_height: 'lineHeight' },
  window: { vertical_tabs: 'verticalTabs' },
  terminal: {
    scrollback_lines: 'scrollbackLines',
    bold_is_bright: 'boldIsBright',
    cursor_style: 'cursorStyle',
    cursor_blink: 'cursorBlink',
  },
  shell: {},
};

/** Tables the server owns; present in the file, ignored by the app. */
export const SERVER_ONLY_TABLES = ['server'] as const;

export const KNOWN_TABLES = [
  ...Object.keys(ConfigSchema.shape),
  ...SERVER_ONLY_TABLES,
] as string[];

/**
 * Rewrite snake_case keys to the canonical camelCase ones and report anything
 * the schema does not know about. Pure: no I/O, no throwing.
 */
export function normalizeRawConfig(raw: unknown): { value: unknown; unknownKeys: string[] } {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    return { value: raw, unknownKeys: [] };
  }
  const unknownKeys: string[] = [];
  const out: Record<string, unknown> = {};
  const shape = ConfigSchema.shape as Record<string, unknown>;

  for (const [table, value] of Object.entries(raw as Record<string, unknown>)) {
    if (!KNOWN_TABLES.includes(table)) {
      unknownKeys.push(table);
      continue;
    }
    if ((SERVER_ONLY_TABLES as readonly string[]).includes(table)) continue;
    if (typeof value !== 'object' || value === null || Array.isArray(value)) {
      out[table] = value;
      continue;
    }
    // `theme` and `keybindings` are free-form records: pass them through.
    if (table === 'theme' || table === 'keybindings') {
      out[table] = value;
      continue;
    }
    const aliases = KEY_ALIASES[table] ?? {};
    const tableShape = shape[table];
    const allowed = tableFieldNames(tableShape);
    const normalized: Record<string, unknown> = {};
    for (const [key, v] of Object.entries(value as Record<string, unknown>)) {
      const canonical = aliases[key] ?? key;
      if (allowed && !allowed.has(canonical)) {
        unknownKeys.push(`${table}.${key}`);
        continue;
      }
      normalized[canonical] = v;
    }
    out[table] = normalized;
  }
  return { value: out, unknownKeys };
}

function tableFieldNames(tableSchema: unknown): Set<string> | null {
  const inner = (tableSchema as { def?: { innerType?: unknown } })?.def?.innerType ?? tableSchema;
  const shape = (inner as { shape?: Record<string, unknown> })?.shape;
  return shape ? new Set(Object.keys(shape)) : null;
}
