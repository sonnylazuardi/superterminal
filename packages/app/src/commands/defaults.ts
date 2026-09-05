/**
 * The v1 command table (05 §5, Q29).
 *
 * Bindings are written once with `mod`; `platform/keys.ts` resolves it to ⌘ on
 * macOS and Ctrl+Shift on Linux. The two entries that genuinely differ per
 * platform (`tab.goto`, `surface.clearScrollback`) spell both out.
 */

import type { SplitAxis } from '@superterminal/protocol-ts';
import type { Platform } from '../platform/detect.js';
import { parseKeybinding, type Keybinding } from '../platform/keys.js';
import { relativeLeaf, siblingLeaf } from '../state/layout.js';
import {
  selectActiveSession,
  selectActiveSurface,
  selectActiveTab,
  selectActiveTabs,
  selectFocusedSurfaceId,
  selectRelativeTab,
  selectTabAt,
  selectTabSurfaces,
} from '../state/selectors.js';
import type { TabView, WorkspaceState } from '../state/types.js';
import type { Command, CommandArg, CommandContext } from './types.js';

/** A binding spec: one string for both platforms, or one per platform. */
export type BindingSpec = string | { darwin: string; other: string };

export function specFor(spec: BindingSpec, platform: Platform): string {
  return typeof spec === 'string' ? spec : platform === 'darwin' ? spec.darwin : spec.other;
}

export function parseSpecs(specs: BindingSpec[], platform: Platform): Keybinding[] {
  return specs.map((s) => parseKeybinding(specFor(s, platform)));
}

const connected = (s: WorkspaceState): boolean => s.connection.status === 'connected';
const hasActiveTab = (s: WorkspaceState): boolean => selectActiveTab(s) !== null;
const hasSurface = (s: WorkspaceState): boolean => selectActiveSurface(s) !== null;
const manyTabs = (s: WorkspaceState): boolean => selectActiveTabs(s).length > 1;
const manyPanes = (s: WorkspaceState): boolean => (selectActiveTab(s)?.surfaceIds.length ?? 0) > 1;

/** `arg` names a Tab (the Menu runs commands on the right-clicked row); else the active one. */
function tabFromArg(state: WorkspaceState, arg: CommandArg | undefined): TabView | null {
  return typeof arg === 'number' ? (state.tabs[arg] ?? null) : selectActiveTab(state);
}

/** Default grid size when nothing is known yet (the server clamps anyway). */
const FALLBACK_COLS = 80;
const FALLBACK_ROWS = 24;

function spawnSpecFromState(state: WorkspaceState, surfaceId?: number | null) {
  const surface =
    surfaceId !== undefined && surfaceId !== null
      ? (state.surfaces[surfaceId] ?? null)
      : selectActiveSurface(state);
  return {
    // Q20: the cwd comes from the server's surface record, never from the
    // client's own process. An exited surface keeps its last known cwd.
    ...(surface?.cwd ? { cwd: surface.cwd } : {}),
    cols: surface?.cols ?? FALLBACK_COLS,
    rows: surface?.rows ?? FALLBACK_ROWS,
  };
}

export interface CommandDefinition {
  id: string;
  title: string;
  bindings: BindingSpec[];
  args?: Array<CommandArg | undefined>;
  when?: (state: WorkspaceState) => boolean;
  hidden?: boolean;
  run: (ctx: CommandContext, arg?: CommandArg) => void | Promise<void>;
}

export const COMMAND_DEFINITIONS: CommandDefinition[] = [
  {
    id: 'tab.new',
    title: 'New Tab',
    bindings: ['mod+t'],
    when: connected,
    async run(ctx) {
      const state = ctx.store.getState();
      const session = state.activeSessionId;
      if (session === null) return;
      await ctx.client.request('tab.create', { session, spawn: spawnSpecFromState(state) });
    },
  },
  {
    id: 'tab.close',
    title: 'Close Tab',
    bindings: ['mod+w'],
    when: hasActiveTab,
    async run(ctx, arg) {
      await closeTab(ctx, tabFromArg(ctx.store.getState(), arg));
    },
  },
  {
    id: 'pane.splitRight',
    title: 'Split Right',
    // `mod+shift+x` collapses onto `mod+x` on Linux/Windows (mod = Ctrl+Shift),
    // so the pane bindings are spelled per platform.
    bindings: [{ darwin: 'mod+d', other: 'ctrl+shift+d' }],
    when: hasActiveTab,
    async run(ctx, arg) {
      await splitPane(ctx, tabFromArg(ctx.store.getState(), arg), 'row');
    },
  },
  {
    id: 'pane.splitDown',
    title: 'Split Down',
    bindings: [{ darwin: 'mod+shift+d', other: 'alt+shift+d' }],
    when: hasActiveTab,
    async run(ctx, arg) {
      await splitPane(ctx, tabFromArg(ctx.store.getState(), arg), 'column');
    },
  },
  {
    id: 'pane.close',
    title: 'Close Pane',
    bindings: [{ darwin: 'mod+shift+w', other: 'alt+shift+w' }],
    when: hasActiveTab,
    async run(ctx, arg) {
      const state = ctx.store.getState();
      const tab = tabFromArg(state, arg);
      if (!tab) return;
      // The last Pane closing IS the Tab closing, confirmation included.
      if (tab.surfaceIds.length <= 1) {
        await closeTab(ctx, tab);
        return;
      }
      const pane = selectFocusedSurfaceId(state, tab.id);
      if (pane === null) return;
      // Hand focus to the surviving sibling before the Pane goes, so the
      // snapshot that removes it never lands on a Tab with stale focus.
      const sibling = siblingLeaf(tab.layout, pane);
      if (sibling !== null) ctx.store.dispatch({ type: 'pane.focus', tabId: tab.id, surfaceId: sibling });
      await ctx.client.request('pane.close', { tab: tab.id, pane });
    },
  },
  {
    id: 'pane.focusNext',
    title: 'Focus Next Pane',
    bindings: [{ darwin: 'mod+]', other: 'alt+]' }],
    when: manyPanes,
    run(ctx) {
      focusRelativePane(ctx, 1);
    },
  },
  {
    id: 'pane.focusPrev',
    title: 'Focus Previous Pane',
    bindings: [{ darwin: 'mod+[', other: 'alt+[' }],
    when: manyPanes,
    run(ctx) {
      focusRelativePane(ctx, -1);
    },
  },
  {
    id: 'tab.next',
    title: 'Next Tab',
    bindings: ['mod+shift+]', 'ctrl+tab'],
    when: manyTabs,
    async run(ctx) {
      const next = selectRelativeTab(ctx.store.getState(), 1);
      if (next) await ctx.client.request('tab.set_active', { tab: next.id });
    },
  },
  {
    id: 'tab.prev',
    title: 'Previous Tab',
    bindings: ['mod+shift+[', 'ctrl+shift+tab'],
    when: manyTabs,
    async run(ctx) {
      const prev = selectRelativeTab(ctx.store.getState(), -1);
      if (prev) await ctx.client.request('tab.set_active', { tab: prev.id });
    },
  },
  {
    id: 'tab.goto',
    title: 'Go to Tab…',
    // ⌘1…⌘9 on macOS, Alt+1…Alt+9 on Linux (plain Ctrl+digit is terminal input).
    bindings: Array.from({ length: 9 }, (_, i) => ({
      darwin: `mod+${i + 1}`,
      other: `alt+${i + 1}`,
    })),
    args: Array.from({ length: 9 }, (_, i) => i + 1),
    hidden: true,
    when: hasActiveTab,
    async run(ctx, arg) {
      const index = typeof arg === 'number' ? arg - 1 : 0;
      const tab = selectTabAt(ctx.store.getState(), index);
      if (tab) await ctx.client.request('tab.set_active', { tab: tab.id });
    },
  },
  {
    id: 'session.new',
    title: 'New Session',
    bindings: ['mod+n'],
    when: connected,
    async run(ctx, arg) {
      const state = ctx.store.getState();
      const name =
        typeof arg === 'string' && arg.trim().length > 0
          ? arg.trim()
          : `Session ${state.sessionOrder.length + 1}`;
      const created = await ctx.client.request('session.create', { name });
      await ctx.client.request('session.set_active', { session: created.session });
      ctx.store.dispatch({ type: 'palette.close' });
    },
  },
  {
    id: 'session.switch',
    title: 'Switch Session…',
    bindings: ['mod+k'],
    when: connected,
    run(ctx) {
      ctx.store.dispatch({ type: 'palette.open', mode: 'sessions' });
    },
  },
  {
    id: 'session.rename',
    title: 'Rename Session',
    bindings: ['mod+r'],
    when: (s) => selectActiveSession(s) !== null,
    run(ctx) {
      const state = ctx.store.getState();
      if (state.activeSessionId === null) return;
      ctx.store.dispatch({ type: 'session.beginRename', sessionId: state.activeSessionId });
    },
  },
  {
    id: 'view.toggleVerticalTabs',
    title: 'Toggle Vertical Tabs',
    bindings: ['mod+shift+b'],
    run(ctx) {
      ctx.store.dispatch({ type: 'ui.toggleVerticalTabs' });
    },
  },
  {
    id: 'edit.copy',
    title: 'Copy',
    bindings: ['mod+c'],
    when: (s) => hasSurface(s),
    run(ctx) {
      const surface = selectActiveSurface(ctx.store.getState());
      if (!surface) return;
      const text = ctx.native.copySelection(surface.id);
      if (text !== null) ctx.store.dispatch({ type: 'toast.push', text: 'Copied' });
    },
  },
  {
    id: 'edit.paste',
    title: 'Paste',
    bindings: ['mod+v'],
    when: hasSurface,
    run(ctx) {
      const surface = selectActiveSurface(ctx.store.getState());
      if (surface) ctx.native.paste(surface.id);
    },
  },
  {
    id: 'surface.clearScrollback',
    title: 'Clear Scrollback',
    bindings: [{ darwin: 'mod+shift+k', other: 'ctrl+shift+l' }],
    when: hasSurface,
    run(ctx) {
      const surface = selectActiveSurface(ctx.store.getState());
      if (surface) ctx.native.clearScrollback(surface.id);
    },
  },
  {
    id: 'palette.commands',
    title: 'Command Palette',
    bindings: ['mod+shift+p'],
    run(ctx) {
      ctx.store.dispatch({ type: 'palette.open', mode: 'commands' });
    },
  },
  {
    id: 'app.reconnect',
    title: 'Reconnect',
    bindings: [],
    when: (s) => s.connection.status !== 'connected',
    async run(ctx) {
      await ctx.app.reconnect();
    },
  },
  {
    id: 'app.quit',
    title: 'Quit',
    bindings: ['mod+q'],
    run(ctx) {
      ctx.app.quit();
    },
  },
];

/** Close a whole Tab, every Pane included (Q21: confirm when anything runs). */
async function closeTab(ctx: CommandContext, tab: TabView | null): Promise<void> {
  if (!tab) return;
  const state = ctx.store.getState();
  const busy = selectTabSurfaces(state, tab.id).some((s) => s.hasForegroundChild);
  if (busy && state.ui.confirmingCloseTabId !== tab.id) {
    ctx.store.dispatch({ type: 'tab.confirmClose', tabId: tab.id });
    return;
  }
  ctx.store.dispatch({ type: 'tab.confirmClose', tabId: null });
  await ctx.client.request('tab.close', { tab: tab.id });
}

/** Split the Tab's focused Pane; the new Pane takes focus once created. */
async function splitPane(ctx: CommandContext, tab: TabView | null, axis: SplitAxis): Promise<void> {
  if (!tab) return;
  const state = ctx.store.getState();
  const pane = selectFocusedSurfaceId(state, tab.id);
  if (pane === null) return;
  const created = await ctx.client.request('tab.split', {
    tab: tab.id,
    pane,
    axis,
    spawn: spawnSpecFromState(state, pane),
  });
  ctx.store.dispatch({ type: 'pane.focus', tabId: tab.id, surfaceId: created.surface });
}

function focusRelativePane(ctx: CommandContext, delta: number): void {
  const state = ctx.store.getState();
  const tab = selectActiveTab(state);
  if (!tab) return;
  const current = selectFocusedSurfaceId(state, tab.id);
  if (current === null) return;
  const next = relativeLeaf(tab.surfaceIds, current, delta);
  if (next !== null) ctx.store.dispatch({ type: 'pane.focus', tabId: tab.id, surfaceId: next });
}

export function toCommand(def: CommandDefinition, platform: Platform, overrides?: string[]): Command {
  const specs: BindingSpec[] = overrides ?? def.bindings;
  const shortcut = parseSpecs(specs, platform);
  return {
    id: def.id,
    title: def.title,
    shortcut,
    // An override replaces the whole list, so the parallel args no longer line
    // up; drop them rather than mis-binding a digit.
    ...(def.args && !overrides ? { shortcutArgs: def.args } : {}),
    ...(def.when ? { when: def.when } : {}),
    ...(def.hidden ? { hidden: def.hidden } : {}),
    run: def.run,
  };
}
