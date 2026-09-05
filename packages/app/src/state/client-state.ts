/**
 * Client State (CONTEXT.md, ADR 0008): what this Client on this machine
 * remembers from its last run — the last window size and the Tab Layout.
 *
 * It is a separate file from `config.toml` on purpose: Config is the user's
 * hand-written declaration and the program never rewrites it, while this
 * file is written by the program and never meant to be edited. When both
 * name the same thing, Client State wins; Config only seeds the first run.
 *
 * Everything here is tolerant: a missing, corrupt or absurd file yields
 * `null`/defaults with a warning, never a crash. The window must always open.
 */

import { mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { z } from 'zod';
import { stateDir, type PathEnv } from '../server/paths.js';
import { SIDEBAR_WIDTH_MAX, SIDEBAR_WIDTH_MIN } from './layout.js';

export const CLIENT_STATE_FILENAME = 'client.json';
export const CLIENT_STATE_VERSION = 1;

/** Anything wider or taller than this is a corrupt file, not a window. */
export const MAX_WINDOW_DIMENSION = 16_384;

const WindowSizeSchema = z.object({
  width: z.number().finite().positive().max(MAX_WINDOW_DIMENSION),
  height: z.number().finite().positive().max(MAX_WINDOW_DIMENSION),
});

/**
 * The file's outer shape only. Each field is validated on its own below so
 * one bad field does not throw the others away.
 */
const ClientStateFileSchema = z.object({
  version: z.number().int().optional(),
  window: z.unknown().optional(),
  verticalTabs: z.unknown().optional(),
  sidebarWidth: z.unknown().optional(),
});

const SidebarWidthSchema = z.number().finite().min(SIDEBAR_WIDTH_MIN).max(SIDEBAR_WIDTH_MAX);

export interface WindowSize {
  width: number;
  height: number;
}

export interface ClientState {
  /** Paintable size in logical pixels, as gpuix reports it. */
  window: WindowSize | null;
  /** Tab Layout: `true` for the sidebar, `false` for the strip. */
  verticalTabs: boolean | null;
  /** Sidebar column width in logical px, within the layout bounds. */
  sidebarWidth: number | null;
}

export const EMPTY_CLIENT_STATE: ClientState = { window: null, verticalTabs: null, sidebarWidth: null };

/** `$XDG_STATE_HOME/superterminal/client.json` (or the platform equivalent). */
export function clientStatePath(input: PathEnv = {}): string {
  return join(stateDir(input), CLIENT_STATE_FILENAME);
}

/**
 * Parse the file's contents. Field-wise: an unusable `window` still leaves
 * `verticalTabs` usable and vice versa. Never throws.
 */
export function parseClientState(text: string): { state: ClientState; warnings: string[] } {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch (err) {
    return {
      state: EMPTY_CLIENT_STATE,
      warnings: [`[superterminal] client state is not valid JSON: ${(err as Error).message}`],
    };
  }
  const parsed = ClientStateFileSchema.safeParse(raw);
  if (!parsed.success) {
    // The whole shape is wrong (an array, a string…): salvage nothing.
    return {
      state: EMPTY_CLIENT_STATE,
      warnings: [`[superterminal] client state ignored: ${parsed.error.issues[0]?.message ?? 'invalid'}`],
    };
  }
  const warnings: string[] = [];
  let window: WindowSize | null = null;
  if (parsed.data.window !== undefined) {
    const size = WindowSizeSchema.safeParse(parsed.data.window);
    if (size.success) window = size.data;
    else warnings.push('[superterminal] client state: remembered window size ignored');
  }
  let verticalTabs: boolean | null = null;
  if (parsed.data.verticalTabs !== undefined) {
    if (typeof parsed.data.verticalTabs === 'boolean') verticalTabs = parsed.data.verticalTabs;
    else warnings.push('[superterminal] client state: remembered tab layout ignored');
  }
  let sidebarWidth: number | null = null;
  if (parsed.data.sidebarWidth !== undefined) {
    const width = SidebarWidthSchema.safeParse(parsed.data.sidebarWidth);
    if (width.success) sidebarWidth = Math.round(width.data);
    else warnings.push('[superterminal] client state: remembered sidebar width ignored');
  }
  return { state: { window, verticalTabs, sidebarWidth }, warnings };
}

export interface LoadClientStateOptions extends PathEnv {
  path?: string;
  readFile?: (path: string) => string;
}

/** Read and parse. A missing file is the normal first run: no warning. */
export function loadClientState(options: LoadClientStateOptions = {}): {
  state: ClientState;
  path: string;
  warnings: string[];
} {
  const path = options.path ?? clientStatePath(options);
  const read = options.readFile ?? ((p: string) => readFileSync(p, 'utf8'));
  let text: string;
  try {
    text = read(path);
  } catch (err) {
    const code = (err as NodeJS.ErrnoException).code;
    return {
      state: EMPTY_CLIENT_STATE,
      path,
      warnings:
        code === 'ENOENT' ? [] : [`[superterminal] could not read ${path}: ${(err as Error).message}`],
    };
  }
  const { state, warnings } = parseClientState(text);
  return { state, path, warnings };
}

/** The exact bytes written; exported so tests can assert on the format. */
export function serializeClientState(state: ClientState): string {
  const file: {
    version: number;
    window?: WindowSize;
    verticalTabs?: boolean;
    sidebarWidth?: number;
  } = { version: CLIENT_STATE_VERSION };
  if (state.window) file.window = state.window;
  if (state.verticalTabs !== null) file.verticalTabs = state.verticalTabs;
  if (state.sidebarWidth !== null) file.sidebarWidth = state.sidebarWidth;
  return `${JSON.stringify(file, null, 2)}\n`;
}

/**
 * Atomic write (temp file + rename) so a crash mid-write leaves the previous
 * state rather than half a JSON document. Synchronous on purpose: the last
 * call happens from `process.on('exit')`, where nothing async runs.
 */
export function saveClientState(path: string, state: ClientState): void {
  mkdirSync(dirname(path), { recursive: true });
  const tmp = `${path}.${process.pid}.tmp`;
  writeFileSync(tmp, serializeClientState(state), 'utf8');
  renameSync(tmp, path);
}

export function sameClientState(a: ClientState, b: ClientState): boolean {
  return (
    a.verticalTabs === b.verticalTabs &&
    a.sidebarWidth === b.sidebarWidth &&
    (a.window === b.window ||
      (a.window !== null &&
        b.window !== null &&
        a.window.width === b.window.width &&
        a.window.height === b.window.height))
  );
}

/**
 * Keeps the file in step with a stream of Client State values.
 *
 * `push` is called on every store change; writes are debounced because a
 * window drag produces one size per frame and the disk only needs the last
 * one. `flush` writes immediately (the exit path) and `stop` cancels the
 * timer (hot reload). Writes are skipped when nothing changed since the
 * last one, so an idle app never touches the disk.
 */
export interface ClientStatePersister {
  push(state: ClientState): void;
  flush(): void;
  stop(): void;
}

export interface PersisterOptions {
  path: string;
  /** What is on disk now, so the first identical push does not rewrite it. */
  initial: ClientState;
  debounceMs?: number;
  write?: (path: string, state: ClientState) => void;
  onError?: (err: unknown) => void;
}

export function createClientStatePersister(options: PersisterOptions): ClientStatePersister {
  const write = options.write ?? saveClientState;
  const debounceMs = options.debounceMs ?? 500;
  let written = options.initial;
  let pending: ClientState | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const commit = (): void => {
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
    if (!pending || sameClientState(pending, written)) {
      pending = null;
      return;
    }
    try {
      write(options.path, pending);
      written = pending;
    } catch (err) {
      options.onError?.(err);
    }
    pending = null;
  };

  return {
    push(state) {
      if (sameClientState(state, pending ?? written)) return;
      pending = state;
      if (timer) clearTimeout(timer);
      timer = setTimeout(commit, debounceMs);
    },
    flush: commit,
    stop() {
      if (timer) clearTimeout(timer);
      timer = null;
      pending = null;
    },
  };
}
