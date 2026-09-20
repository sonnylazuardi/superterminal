/**
 * The unified palette's rows (08 Q1/Q2): commands, tabs and sessions, built
 * from the store without gpuix so they are testable. The component only
 * paints what this returns and wires `activate`.
 *
 * A Tab is also findable by what it *shows* (08 §H): when the palette has
 * fetched screen text (`use-screen-text.ts`), a query that appears nowhere in
 * a title but does appear on that tab's screen still matches, one tier below
 * every title match so titles never lose their place. The row then carries
 * the matching line as its hint, because a match the user cannot see the
 * reason for looks like a bug.
 */

import { filterCommands, fuzzyScore, type Command } from '../commands/registry.js';
import { excerptFor, matchesScreen, screenSnippet } from '../ai/screen-context.js';
import { selectActiveTabId, selectActiveTabs, selectSessions, selectSurfaceForTab } from '../state/selectors.js';
import { displayTitle } from '../state/title.js';
import type { PaletteMode, SessionView, TabView, WorkspaceState } from '../state/types.js';
import type { Candidate } from '../ai/palette-rank.js';

export type RowKind = 'command' | 'tab' | 'session' | 'action';

export interface PaletteRow {
  key: string;
  kind: RowKind;
  title: string;
  hint: string;
  /** What Jev reads for this row; empty for pure UI rows. */
  label: string;
  target:
    | { type: 'command'; id: string }
    | { type: 'tab'; id: number }
    | { type: 'session'; id: number }
    | { type: 'session.new'; name: string }
    | { type: 'ai.settings' };
}

export interface BuildRowsInput {
  state: WorkspaceState;
  commands: Command[];
  mode: PaletteMode;
  query: string;
  shortcutHint: (id: string) => string;
  /** Visible lines per Surface id, when the palette has them (08 §H). */
  screenText?: Map<number, string[]>;
  /** `ui.ai.screenContext`: may screen text be sent to the model? */
  screenContext?: boolean;
}

/**
 * The score of a match found only on screen content. `fuzzyScore` returns at
 * worst (matched characters) − (target length)/100, so a value this far down
 * puts every content match in its own tier under every title match, while
 * keeping content matches in tab order among themselves.
 */
export const CONTENT_MATCH_SCORE = -1000;

/** Marks a row that matched on what its screen shows, not on its title. */
export const CONTENT_HINT_PREFIX = '⌕ ';

export interface BuiltRows {
  /** Fuzzy-matched rows, best first (what shows before Jev answers). */
  local: PaletteRow[];
  /** Every row the query could resolve to, by key (what Jev may pick from). */
  all: Map<string, PaletteRow>;
  candidates: Candidate[];
  /** The active tab described the way candidates are, for the request state. */
  activeTabLabel: string | null;
}

export function tabTitle(state: WorkspaceState, tab: TabView): string {
  const surface = selectSurfaceForTab(state, tab.id);
  return displayTitle(surface?.title, 'shell');
}

/** `Tab 3 · frontend · ~/projects/app · session Work · busy` (08 Q2). */
export function tabLabel(state: WorkspaceState, tab: TabView, index: number): string {
  const surface = selectSurfaceForTab(state, tab.id);
  const session = state.sessions[tab.sessionId];
  const parts = [`Tab ${index + 1}`, tabTitle(state, tab)];
  if (surface?.cwd) parts.push(shortenHome(surface.cwd));
  if (session) parts.push(`session ${session.name}`);
  if (surface?.status === 'exited') parts.push('exited');
  else if (surface?.hasForegroundChild) parts.push('busy');
  if (tab.surfaceIds.length > 1) parts.push(`${tab.surfaceIds.length} panes`);
  if (selectActiveTabId(state) === tab.id) parts.push('active');
  return parts.join(' · ');
}

/**
 * The tab label the model reads. Screen text is appended only when the
 * screen-context toggle is on (it leaves the machine, so it is opt-in); with
 * it off the label is byte-for-byte what it has always been.
 */
function tabRowLabel(state: WorkspaceState, tab: TabView, index: number, screens: string[][]): string {
  const label = tabLabel(state, tab, index);
  if (screens.length === 0) return label;
  const excerpts = screens.map((lines) => excerptFor(lines)).filter((text) => text.trim().length > 0);
  if (excerpts.length === 0) return label;
  return `${label}\nscreen: ${excerpts.join('\n---\n')}`;
}

export function sessionLabel(state: WorkspaceState, session: SessionView): string {
  const n = session.tabIds.length;
  const parts = [`Session ${session.name}`, `${n} tab${n === 1 ? '' : 's'}`];
  if (state.activeSessionId === session.id) parts.push('active');
  return parts.join(' · ');
}

function shortenHome(path: string): string {
  const home = process.env['HOME'] ?? process.env['USERPROFILE'];
  return home && path.startsWith(home) ? `~${path.slice(home.length)}` : path;
}

export function buildRows(input: BuildRowsInput): BuiltRows {
  const { state, mode, query, shortcutHint } = input;
  const all = new Map<string, PaletteRow>();
  const candidates: Candidate[] = [];
  const local: PaletteRow[] = [];
  const q = query.trim();

  const sessions = selectSessions(state);
  const activeSessionId = state.activeSessionId;

  // Sessions.
  const sessionRows = sessions.map((session) => {
    const row: PaletteRow = {
      key: `session:${session.id}`,
      kind: 'session',
      title: session.name,
      hint: session.id === activeSessionId ? 'session · active' : 'session',
      label: sessionLabel(state, session),
      target: { type: 'session', id: session.id },
    };
    all.set(row.key, row);
    candidates.push({ id: row.key, kind: 'session', label: row.label });
    return { row, score: fuzzyScore(q, session.name) };
  });

  if (mode === 'sessions') {
    local.push(
      ...sessionRows
        .filter((r): r is { row: PaletteRow; score: number } => r.score !== null)
        .sort((a, b) => b.score - a.score)
        .map((r) => r.row),
    );
    const exact = sessions.some((s) => s.name.toLowerCase() === q.toLowerCase());
    if (q.length > 0 && !exact) local.push(newSessionRow(q, shortcutHint('session.new')));
    return { local, all, candidates, activeTabLabel: null };
  }

  // Commands (enabled and not hidden), in table order, fuzzy-scored on title.
  const commandRows = filterCommands(input.commands, q, state).map(({ command, score }) => {
    const row = commandRow(command, shortcutHint(command.id));
    return { row, score };
  });
  for (const command of input.commands) {
    if (command.hidden) continue;
    if (command.when && !command.when(state)) continue;
    const row = commandRow(command, shortcutHint(command.id));
    all.set(row.key, row);
    candidates.push({ id: row.key, kind: 'command', label: row.label });
  }

  // Tabs of the active session; fuzzy on title, cwd tail and session name,
  // and — when the palette has screen text — on what the panes show.
  const tabs = selectActiveTabs(state);
  let activeTabLabel: string | null = null;
  const tabRows = tabs.map((tab, index) => {
    const surface = selectSurfaceForTab(state, tab.id);
    const title = tabTitle(state, tab);
    const cwd = surface?.cwd ? shortenHome(surface.cwd) : '';
    const screens = tab.surfaceIds
      .map((id) => input.screenText?.get(id))
      .filter((lines): lines is string[] => Boolean(lines && lines.length > 0));

    const titleScore = q.length === 0 ? 0 : bestScore(q, [title, cwd, `tab ${index + 1}`]);
    // Only look at the screen when nothing in the title matched: a title match
    // already ranks higher, and its own hint (cwd, state) is the useful one.
    const hit = titleScore === null && q.length > 0 ? screens.find((lines) => matchesScreen(q, lines)) : undefined;
    const snippet = hit ? screenSnippet(q, hit) : null;

    const row: PaletteRow = {
      key: `tab:${tab.id}`,
      kind: 'tab',
      title,
      hint: snippet
        ? `${CONTENT_HINT_PREFIX}${snippet}`
        : [cwd && cwd !== title ? cwd : '', tabHintState(state, tab)].filter(Boolean).join(' · ') || 'tab',
      label: tabRowLabel(state, tab, index, input.screenContext === true ? screens : []),
      target: { type: 'tab', id: tab.id },
    };
    all.set(row.key, row);
    candidates.push({ id: row.key, kind: 'tab', label: row.label });
    if (selectActiveTabId(state) === tab.id) activeTabLabel = row.label;
    const score = titleScore ?? (hit ? CONTENT_MATCH_SCORE : null);
    return { row, score };
  });

  const scored: Array<{ row: PaletteRow; score: number }> = [];
  for (const r of commandRows) scored.push(r);
  for (const r of tabRows) if (r.score !== null) scored.push({ row: r.row, score: r.score });
  if (q.length > 0) {
    for (const r of sessionRows) if (r.score !== null) scored.push({ row: r.row, score: r.score });
  }
  // Stable: commands first on an empty query, then tabs; sessions only when searched.
  scored.sort((a, b) => b.score - a.score);
  local.push(...scored.map((r) => r.row));

  return { local, all, candidates, activeTabLabel };
}

function bestScore(query: string, targets: string[]): number | null {
  let best: number | null = null;
  for (const t of targets) {
    if (!t) continue;
    const s = fuzzyScore(query, t);
    if (s !== null && (best === null || s > best)) best = s;
  }
  return best;
}

function tabHintState(state: WorkspaceState, tab: TabView): string {
  const surface = selectSurfaceForTab(state, tab.id);
  if (selectActiveTabId(state) === tab.id) return 'active';
  if (surface?.status === 'exited') return 'exited';
  if (surface?.hasForegroundChild) return 'busy';
  return '';
}

function commandRow(command: Command, hint: string): PaletteRow {
  return {
    key: `cmd:${command.id}`,
    kind: 'command',
    title: command.title,
    hint,
    label: `${command.title} — ${command.description}`,
    target: { type: 'command', id: command.id },
  };
}

export function newSessionRow(name: string, hint: string): PaletteRow {
  return {
    key: 'session-new',
    kind: 'action',
    title: `New Session “${name}”`,
    hint,
    label: '',
    target: { type: 'session.new', name },
  };
}

export function aiOffRow(): PaletteRow {
  return {
    key: 'ai-off',
    kind: 'action',
    title: 'No matches · AI ranking is off — set up a key',
    hint: 'AI Settings…',
    label: '',
    target: { type: 'ai.settings' },
  };
}
