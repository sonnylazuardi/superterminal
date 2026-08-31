import type { ReqParams, RequestType, ResOk } from '@superterminal/protocol-ts';
import type { Platform } from '../platform/detect.js';
import type { Keybinding } from '../platform/keys.js';
import type { WorkspaceState } from '../state/types.js';
import type { WorkspaceStore } from '../state/workspace-store.js';

/** The slice of `ControlClient` commands need. Keeps them mockable. */
export interface ControlClientLike {
  request<M extends RequestType>(
    type: M,
    params: ReqParams<M>,
    opts?: { timeoutMs?: number },
  ): Promise<ResOk<M>>;
  readonly state: 'connecting' | 'connected' | 'reconnecting' | 'closed';
}

/**
 * Imperative operations that belong to the native `<terminal-grid>` (04 §3):
 * copy, paste and clear-scrollback are one-shot commands on the element, not
 * state. The app owns a small adapter; commands only see this interface.
 */
export interface NativeBridge {
  hasSelection(surfaceId: number): boolean;
  /** Returns the copied text, or null when there was no selection. */
  copySelection(surfaceId: number): string | null;
  paste(surfaceId: number): void;
  clearScrollback(surfaceId: number): void;
  focus(surfaceId: number): void;
}

/** Whole-app actions that are neither the store nor the socket. */
export interface AppBridge {
  reconnect(): void | Promise<void>;
  quit(): void;
}

export interface CommandContext {
  store: WorkspaceStore;
  client: ControlClientLike;
  native: NativeBridge;
  app: AppBridge;
  platform: Platform;
}

export type CommandArg = number | string;

export interface Command {
  /** Stable id, e.g. `tab.new`. Also the key for config keybinding overrides. */
  id: string;
  title: string;
  /** Platform-resolved at registry build time. */
  shortcut: Keybinding[];
  /**
   * Argument bound to `shortcut[i]`, for commands that take one (`tab.goto` is
   * one command with a numeric argument, so the palette stays clean — 05 §5).
   */
  shortcutArgs?: Array<CommandArg | undefined>;
  /** Enablement; a disabled command is also hidden from the palette. */
  when?: (state: WorkspaceState) => boolean;
  /** Bound to keys but never listed in the palette. */
  hidden?: boolean;
  run: (ctx: CommandContext, arg?: CommandArg) => void | Promise<void>;
}

export interface CommandMatch {
  command: Command;
  arg?: CommandArg;
}

export const noopNativeBridge: NativeBridge = {
  hasSelection: () => false,
  copySelection: () => null,
  paste: () => {},
  clearScrollback: () => {},
  focus: () => {},
};

export const noopAppBridge: AppBridge = {
  reconnect: () => {},
  quit: () => {},
};
