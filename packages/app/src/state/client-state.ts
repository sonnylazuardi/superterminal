/**
 * Client State (CONTEXT.md, ADR 0008): what this Client on this machine
 * remembers from its last run — the Window Placement and the Tab Layout.
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
import { clampFontZoom } from './zoom.js';
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

/** The origin is valid only as a pair; a lone `x` is a corrupt placement. */
const WindowOriginSchema = z.object({
  x: z.number().finite(),
  y: z.number().finite(),
});

const WindowDisplayBoundsSchema = z.object({
  x: z.number().finite(),
  y: z.number().finite(),
  width: z.number().finite().positive().max(MAX_WINDOW_DIMENSION),
  height: z.number().finite().positive().max(MAX_WINDOW_DIMENSION),
});

/** The display object's shape only; `uuid` and `bounds` parse field-wise. */
const WindowDisplayFileSchema = z.object({
  uuid: z.unknown().optional(),
  bounds: z.unknown().optional(),
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
  fontZoom: z.unknown().optional(),
});

const SidebarWidthSchema = z.number().finite().min(SIDEBAR_WIDTH_MIN).max(SIDEBAR_WIDTH_MAX);
const FontZoomSchema = z.number().finite();

export interface WindowSize {
  width: number;
  height: number;
}

/** A connected display's frame, in logical pixels (gpuix's `DisplayBounds`). */
export interface WindowDisplayBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * The display a window was on. `uuid` is the stable identifier when the
 * platform has one; `bounds` is the fallback for platforms that do not.
 */
export interface WindowDisplay {
  uuid?: string;
  bounds?: WindowDisplayBounds;
}

/**
 * The Window Placement (CONTEXT.md): size plus the optional origin, display
 * and maximized state. Only `width`/`height` are required, so a v1 file (or a
 * window whose position read failed) still restores its size.
 */
export interface WindowPlacement extends WindowSize {
  x?: number;
  y?: number;
  maximized?: boolean;
  display?: WindowDisplay;
}

export interface ClientState {
  /** The Window Placement, as gpuix reports it in logical pixels. */
  window: WindowPlacement | null;
  /** Tab Layout: `true` for the sidebar, `false` for the strip. */
  verticalTabs: boolean | null;
  /** Sidebar column width in logical px, within the layout bounds. */
  sidebarWidth: number | null;
  /** ⌘+ / ⌘− font zoom, in points added to the configured size. */
  fontZoom: number | null;
}

export const EMPTY_CLIENT_STATE: ClientState = {
  window: null,
  verticalTabs: null,
  sidebarWidth: null,
  fontZoom: null,
};

/** `$XDG_STATE_HOME/superterminal/client.json` (or the platform equivalent). */
export function clientStatePath(input: PathEnv = {}): string {
  return join(stateDir(input), CLIENT_STATE_FILENAME);
}

/**
 * One window record, field by field. A bad size throws the record away; a bad
 * `x` drops only the origin, so a corrupt position still restores the size
 * (and, if they are intact, the display and maximized state).
 */
function parseWindow(raw: unknown, warnings: string[]): WindowPlacement | null {
  const size = WindowSizeSchema.safeParse(raw);
  if (!size.success) {
    warnings.push('[superterminal] client state: remembered window size ignored');
    return null;
  }
  const record = raw as Record<string, unknown>;
  const window: WindowPlacement = { width: size.data.width, height: size.data.height };

  if (record['x'] !== undefined || record['y'] !== undefined) {
    const origin = WindowOriginSchema.safeParse({ x: record['x'], y: record['y'] });
    if (origin.success) {
      window.x = origin.data.x;
      window.y = origin.data.y;
    } else {
      warnings.push('[superterminal] client state: remembered window position ignored');
    }
  }

  if (record['maximized'] !== undefined) {
    if (typeof record['maximized'] === 'boolean') window.maximized = record['maximized'];
    else warnings.push('[superterminal] client state: remembered maximized state ignored');
  }

  const display = parseWindowDisplay(record['display'], warnings);
  if (display) window.display = display;

  return window;
}

function parseWindowDisplay(raw: unknown, warnings: string[]): WindowDisplay | undefined {
  if (raw === undefined) return undefined;
  const parsed = WindowDisplayFileSchema.safeParse(raw);
  const display: WindowDisplay = {};
  if (parsed.success && typeof parsed.data.uuid === 'string' && parsed.data.uuid.length > 0) {
    display.uuid = parsed.data.uuid;
  }
  const bounds = parsed.success
    ? WindowDisplayBoundsSchema.safeParse(parsed.data.bounds)
    : undefined;
  if (bounds?.success) display.bounds = bounds.data;
  if (display.uuid === undefined && display.bounds === undefined) {
    warnings.push('[superterminal] client state: remembered display ignored');
    return undefined;
  }
  return display;
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
  const window = parsed.data.window !== undefined ? parseWindow(parsed.data.window, warnings) : null;
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
  let fontZoom: number | null = null;
  if (parsed.data.fontZoom !== undefined) {
    const zoom = FontZoomSchema.safeParse(parsed.data.fontZoom);
    if (zoom.success) fontZoom = clampFontZoom(zoom.data);
    else warnings.push('[superterminal] client state: remembered font zoom ignored');
  }

  return { state: { window, verticalTabs, sidebarWidth, fontZoom }, warnings };
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

/**
 * The window record as it is written. `maximized: false` is the default and is
 * omitted, so a never-maximized file does not change shape across runs.
 */
function serializeWindow(window: WindowPlacement): WindowPlacement {
  const file: WindowPlacement = { width: window.width, height: window.height };
  if (window.x !== undefined && window.y !== undefined) {
    file.x = window.x;
    file.y = window.y;
  }
  if (window.maximized) file.maximized = true;
  if (window.display) file.display = window.display;
  return file;
}

/** The exact bytes written; exported so tests can assert on the format. */
export function serializeClientState(state: ClientState): string {
  const file: {
    version: number;
    window?: WindowPlacement;
    verticalTabs?: boolean;
    sidebarWidth?: number;
    fontZoom?: number;
  } = { version: CLIENT_STATE_VERSION };
  if (state.window) file.window = serializeWindow(state.window);
  if (state.verticalTabs !== null) file.verticalTabs = state.verticalTabs;
  if (state.sidebarWidth !== null) file.sidebarWidth = state.sidebarWidth;
  // Zero is the default; not writing it keeps a never-zoomed file unchanged.
  if (state.fontZoom !== null && state.fontZoom !== 0) file.fontZoom = state.fontZoom;
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

/** Field-wise equality of two placements; `undefined` and `false` agree. */
export function sameWindowPlacement(
  a: WindowPlacement | null,
  b: WindowPlacement | null,
): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return (
    a.width === b.width &&
    a.height === b.height &&
    a.x === b.x &&
    a.y === b.y &&
    (a.maximized ?? false) === (b.maximized ?? false) &&
    sameWindowDisplay(a.display, b.display)
  );
}

function sameWindowDisplay(a: WindowDisplay | undefined, b: WindowDisplay | undefined): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  if (a.uuid !== b.uuid) return false;
  const aBounds = a.bounds;
  const bBounds = b.bounds;
  if (aBounds === bBounds) return true;
  if (!aBounds || !bBounds) return false;
  return (
    aBounds.x === bBounds.x &&
    aBounds.y === bBounds.y &&
    aBounds.width === bBounds.width &&
    aBounds.height === bBounds.height
  );
}

export function sameClientState(a: ClientState, b: ClientState): boolean {
  return (
    a.verticalTabs === b.verticalTabs &&
    a.sidebarWidth === b.sidebarWidth &&
    (a.fontZoom ?? 0) === (b.fontZoom ?? 0) &&
    sameWindowPlacement(a.window, b.window)
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
