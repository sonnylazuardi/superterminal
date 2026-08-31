/**
 * Command registry (05 §5, Q29).
 *
 * Owns the platform-resolved keybinding table, the palette's command list and
 * the `passthroughShortcuts` array handed to `<terminal-grid>` — one source of
 * truth for all three, so a shortcut the element declines is exactly a shortcut
 * the app can run.
 */

import type { Platform } from '../platform/detect.js';
import {
  bindingMatchesEvent,
  formatKeybinding,
  toKeystrokeString,
  type KeyEventLike,
  type Keybinding,
} from '../platform/keys.js';
import type { WorkspaceState } from '../state/types.js';
import { COMMAND_DEFINITIONS, toCommand, type CommandDefinition } from './defaults.js';
import type { Command, CommandArg, CommandContext, CommandMatch } from './types.js';

export type { Command, CommandArg, CommandContext, CommandMatch } from './types.js';
export { noopAppBridge, noopNativeBridge } from './types.js';

export interface BuildRegistryOptions {
  platform: Platform;
  /**
   * `config.keybindings`: commandId -> `"mod+shift+t"`, or several bindings
   * separated by commas. An empty string unbinds the command.
   */
  overrides?: Record<string, string>;
  /** Replace the built-in table (tests). */
  definitions?: CommandDefinition[];
  /** Warnings for unparsable overrides / unknown ids. */
  onWarning?: (message: string) => void;
}

export interface CommandRegistry {
  readonly platform: Platform;
  readonly commands: Command[];
  byId(id: string): Command | undefined;
  /** Commands the palette should list for this state, in table order. */
  visible(state: WorkspaceState): Command[];
  isEnabled(command: Command, state: WorkspaceState): boolean;
  /**
   * First enabled command bound to this key event, or null. `state` is optional
   * so a keystroke can be matched without a store (tests, tooling); without it
   * `when` is not consulted.
   */
  matchKeybinding(event: KeyEventLike, state?: WorkspaceState): CommandMatch | null;
  /** Palette hint, e.g. `⌘⇧P` / `Ctrl+Shift+P`. */
  shortcutHint(id: string): string;
  /** gpuix keystroke strings the `<terminal-grid>` must decline (05 §5). */
  readonly passthroughShortcuts: string[];
  run(id: string, ctx: CommandContext, arg?: CommandArg): void | Promise<void>;
}

function splitOverride(spec: string): string[] {
  return spec
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

export function buildRegistry(options: BuildRegistryOptions): CommandRegistry {
  const { platform, overrides = {}, definitions = COMMAND_DEFINITIONS } = options;
  const warn = options.onWarning ?? ((m: string) => process.stderr.write(`${m}\n`));

  const knownIds = new Set(definitions.map((d) => d.id));
  for (const id of Object.keys(overrides)) {
    if (!knownIds.has(id)) warn(`[superterminal] unknown command id in keybindings: ${id}`);
  }

  const commands: Command[] = definitions.map((def) => {
    const override = overrides[def.id];
    if (override === undefined) return toCommand(def, platform);
    try {
      return toCommand(def, platform, splitOverride(override));
    } catch (err) {
      warn(`[superterminal] bad keybinding for ${def.id}: ${(err as Error).message}`);
      return toCommand(def, platform);
    }
  });

  const byId = new Map(commands.map((c) => [c.id, c] as const));

  const passthroughShortcuts = [
    ...new Set(
      commands.flatMap((c) => c.shortcut.map((b: Keybinding) => toKeystrokeString(b, platform))),
    ),
  ];

  const isEnabled = (command: Command, state: WorkspaceState): boolean =>
    command.when ? command.when(state) : true;

  return {
    platform,
    commands,
    byId: (id) => byId.get(id),
    visible: (state) => commands.filter((c) => !c.hidden && isEnabled(c, state)),
    isEnabled,
    matchKeybinding(event, state) {
      for (const command of commands) {
        for (let i = 0; i < command.shortcut.length; i++) {
          const binding = command.shortcut[i]!;
          if (!bindingMatchesEvent(binding, event, platform)) continue;
          if (state && !isEnabled(command, state)) continue;
          const arg = command.shortcutArgs?.[i];
          return arg === undefined ? { command } : { command, arg };
        }
      }
      return null;
    },
    shortcutHint(id) {
      const command = byId.get(id);
      const binding = command?.shortcut[0];
      return binding ? formatKeybinding(binding, platform) : '';
    },
    passthroughShortcuts,
    run(id, ctx, arg) {
      const command = byId.get(id);
      if (!command) {
        warn(`[superterminal] no such command: ${id}`);
        return;
      }
      return command.run(ctx, arg);
    },
  };
}

/**
 * Standalone matcher for a list of commands — the registry method delegates to
 * the same logic, exposed for callers that hold a bare array.
 */
export function matchKeybinding(
  commands: Command[],
  event: KeyEventLike,
  platform: Platform,
  state?: WorkspaceState,
): CommandMatch | null {
  for (const command of commands) {
    for (let i = 0; i < command.shortcut.length; i++) {
      const binding = command.shortcut[i]!;
      if (!bindingMatchesEvent(binding, event, platform)) continue;
      if (state && command.when && !command.when(state)) continue;
      const arg = command.shortcutArgs?.[i];
      return arg === undefined ? { command } : { command, arg };
    }
  }
  return null;
}

/**
 * Subsequence fuzzy scorer for the palette (05 §4): higher is better, `null`
 * when the query is not a subsequence of the title.
 */
export function fuzzyScore(query: string, target: string): number | null {
  if (query.length === 0) return 0;
  const q = query.toLowerCase();
  const t = target.toLowerCase();
  let score = 0;
  let ti = 0;
  let previousMatch = -2;
  for (let qi = 0; qi < q.length; qi++) {
    const ch = q[qi]!;
    if (ch === ' ') continue;
    const found = t.indexOf(ch, ti);
    if (found < 0) return null;
    score += 1;
    if (found === previousMatch + 1) score += 3; // contiguous run
    if (found === 0 || t[found - 1] === ' ' || t[found - 1] === '.') score += 2; // word start
    previousMatch = found;
    ti = found + 1;
  }
  // Prefer shorter titles for the same match quality.
  return score - target.length * 0.01;
}

export interface ScoredCommand {
  command: Command;
  score: number;
}

export function filterCommands(
  commands: Command[],
  query: string,
  state?: WorkspaceState,
): ScoredCommand[] {
  const out: ScoredCommand[] = [];
  for (const command of commands) {
    if (command.hidden) continue;
    if (state && command.when && !command.when(state)) continue;
    const score = fuzzyScore(query, command.title);
    if (score === null) continue;
    out.push({ command, score });
  }
  return out.sort((a, b) => b.score - a.score);
}
