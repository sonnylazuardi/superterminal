/**
 * Keybinding parsing, platform resolution and matching (05 §5, Q29).
 *
 * `mod` is the app-shortcut modifier: ⌘ on macOS. On Linux plain Ctrl+letter is
 * a terminal keystroke (Ctrl+T transposes in readline), so `mod` resolves to
 * **Ctrl+Shift** there. The default table is written once with `mod`.
 */

import type { Platform } from './detect.js';

export type Modifier = 'mod' | 'shift' | 'alt' | 'ctrl';

export interface Keybinding {
  mods: Modifier[];
  /** Lower-case gpuix key name: `t`, `1`, `tab`, `enter`, `[`, `]`. */
  key: string;
}

/** A binding with `mod` expanded for one platform. */
export interface ResolvedBinding {
  cmd: boolean;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  key: string;
}

export interface KeyEventLike {
  key?: string;
  modifiers?: { shift?: boolean; ctrl?: boolean; alt?: boolean; cmd?: boolean };
}

/** Aliases accepted for the key part of a binding. */
const KEY_ALIASES: Record<string, string> = {
  esc: 'escape',
  return: 'enter',
  del: 'delete',
  ins: 'insert',
  pgup: 'pageup',
  pgdn: 'pagedown',
  spacebar: 'space',
};

/** Aliases accepted for the modifier part of a binding. */
const MOD_ALIASES: Record<string, Modifier> = {
  mod: 'mod',
  cmd: 'mod',
  meta: 'mod',
  super: 'mod',
  win: 'mod',
  shift: 'shift',
  alt: 'alt',
  option: 'alt',
  opt: 'alt',
  ctrl: 'ctrl',
  control: 'ctrl',
};

export function normalizeKey(key: string): string {
  const k = key.trim();
  if (k.length === 1) return k.toLowerCase();
  const lower = k.toLowerCase();
  return KEY_ALIASES[lower] ?? lower;
}

/** Parse `"mod+shift+t"` / `"ctrl-tab"` / `"alt+1"`. Throws on nonsense. */
export function parseKeybinding(spec: string): Keybinding {
  const parts = spec
    .split(/[+-](?=.)/)
    .map((p) => p.trim())
    .filter(Boolean);
  if (parts.length === 0) throw new Error(`empty keybinding: ${JSON.stringify(spec)}`);
  const key = normalizeKey(parts.pop()!);
  const mods: Modifier[] = [];
  for (const part of parts) {
    const canonical = MOD_ALIASES[part.toLowerCase()];
    if (!canonical) {
      throw new Error(`unknown modifier ${JSON.stringify(part)} in ${JSON.stringify(spec)}`);
    }
    if (!mods.includes(canonical)) mods.push(canonical);
  }
  return { mods, key };
}

/** Expand `mod` for the given platform. */
export function resolveBinding(binding: Keybinding, platform: Platform): ResolvedBinding {
  const has = (m: Modifier) => binding.mods.includes(m);
  const mod = has('mod');
  const isMac = platform === 'darwin';
  return {
    cmd: mod && isMac,
    // Linux/Windows: mod = Ctrl+Shift (Q29).
    ctrl: has('ctrl') || (mod && !isMac),
    alt: has('alt'),
    shift: has('shift') || (mod && !isMac),
    key: normalizeKey(binding.key),
  };
}

/**
 * gpuix/GPUI keystroke string, modifiers in `ctrl-alt-shift-cmd` order:
 * `"cmd-t"`, `"ctrl-shift-t"`, `"ctrl-tab"`.
 */
export function toKeystrokeString(binding: Keybinding, platform: Platform): string {
  const r = resolveBinding(binding, platform);
  const parts: string[] = [];
  if (r.ctrl) parts.push('ctrl');
  if (r.alt) parts.push('alt');
  if (r.shift) parts.push('shift');
  if (r.cmd) parts.push('cmd');
  parts.push(r.key);
  return parts.join('-');
}

const DISPLAY_KEYS: Record<string, string> = {
  enter: '↵',
  escape: 'Esc',
  tab: '⇥',
  space: 'Space',
  up: '↑',
  down: '↓',
  left: '←',
  right: '→',
  backspace: '⌫',
  delete: '⌦',
};

/** Human-readable hint for the palette: `⌘⇧P` / `Ctrl+Shift+P`. */
export function formatKeybinding(binding: Keybinding, platform: Platform): string {
  const r = resolveBinding(binding, platform);
  const key = DISPLAY_KEYS[r.key] ?? (r.key.length === 1 ? r.key.toUpperCase() : titleCase(r.key));
  if (platform === 'darwin') {
    return `${r.ctrl ? '⌃' : ''}${r.alt ? '⌥' : ''}${r.shift ? '⇧' : ''}${r.cmd ? '⌘' : ''}${key}`;
  }
  const parts: string[] = [];
  if (r.ctrl) parts.push('Ctrl');
  if (r.alt) parts.push('Alt');
  if (r.shift) parts.push('Shift');
  parts.push(key);
  return parts.join('+');
}

function titleCase(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** Exact modifier match — an extra modifier never triggers a command. */
export function bindingMatchesEvent(
  binding: Keybinding,
  event: KeyEventLike,
  platform: Platform,
): boolean {
  if (!event.key) return false;
  const r = resolveBinding(binding, platform);
  const m = event.modifiers ?? {};
  return (
    normalizeKey(event.key) === r.key &&
    Boolean(m.cmd) === r.cmd &&
    Boolean(m.ctrl) === r.ctrl &&
    Boolean(m.alt) === r.alt &&
    Boolean(m.shift) === r.shift
  );
}

export function keybindingEquals(a: Keybinding, b: Keybinding, platform: Platform): boolean {
  const ra = resolveBinding(a, platform);
  const rb = resolveBinding(b, platform);
  return (
    ra.key === rb.key &&
    ra.cmd === rb.cmd &&
    ra.ctrl === rb.ctrl &&
    ra.alt === rb.alt &&
    ra.shift === rb.shift
  );
}
