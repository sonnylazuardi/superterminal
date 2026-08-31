/**
 * Command palette (05 §4): one `<anchored>` at top-center, 560 px wide, with an
 * `<input>` and a plain `<div>` list capped at 8 visible rows (nested scrolling
 * is unsupported, so the window of items shifts instead of scrolling).
 *
 * Modes: `commands` (⌘⇧P / Ctrl+Shift+P) and `sessions` (⌘K / Ctrl+Shift+K).
 * In sessions mode a trailing row offers **New Session "‹query›"** when the
 * query matches no existing name.
 */

import { filterCommands, type Command } from '../commands/registry.js';
import { fuzzyScore } from '../commands/registry.js';
import { selectSessions } from '../state/selectors.js';
import type { SessionView } from '../state/types.js';
import { useFullWorkspace, useRunCommand, useServices, useWorkspace } from './context.js';

const WIDTH = 560;
const MAX_ROWS = 8;

interface Row {
  key: string;
  title: string;
  hint: string;
  activate: () => void;
}

export function CommandPalette() {
  const { tokens, registry, store, commandContext } = useServices();
  const state = useFullWorkspace();
  const open = state.ui.paletteOpen;
  const mode = state.ui.paletteMode;
  const query = state.ui.paletteQuery;
  const index = state.ui.paletteIndex;
  const sessions = useWorkspace(selectSessions);
  const run = useRunCommand();

  if (!open) return null;

  const rows: Row[] =
    mode === 'commands'
      ? filterCommands(registry.commands, query, state).map(({ command }) => ({
          key: command.id,
          title: command.title,
          hint: registry.shortcutHint(command.id),
          activate: () => {
            store.dispatch({ type: 'palette.close' });
            run(command.id);
          },
        }))
      : sessionRows(sessions, query, state.activeSessionId);

  const clamped = Math.min(index, Math.max(0, rows.length - 1));
  // No inner scroll container: shift the window of items instead.
  const start = Math.max(0, Math.min(clamped - MAX_ROWS + 1, rows.length - MAX_ROWS));
  const visible = rows.slice(Math.max(0, start), Math.max(0, start) + MAX_ROWS);

  return (
    <anchored
      testId="command-palette"
      anchor="topCenter"
      offset={{ x: 0, y: 80 }}
      style={{
        width: WIDTH,
        flexDirection: 'column',
        backgroundColor: tokens.bg.overlay,
        borderRadius: tokens.radius.panel,
        borderWidth: tokens.border.width,
        borderColor: tokens.border.glass,
        padding: 8,
        gap: 4,
      }}
    >
      <input
        testId="palette-input"
        autoFocus
        value={query}
        placeholder={mode === 'commands' ? 'Run a command…' : 'Switch or create a session…'}
        style={{
          height: 32,
          paddingLeft: 10,
          paddingRight: 10,
          borderRadius: tokens.radius.tab,
          backgroundColor: tokens.bg.glass,
          borderWidth: tokens.border.width,
          borderColor: tokens.accent,
          color: tokens.fg.primary,
          fontSize: tokens.font.paletteInput,
        }}
        onChange={(event) =>
          store.dispatch({ type: 'palette.setQuery', query: String(event.value ?? '') })
        }
        onKeyDown={(event) => {
          switch (event.key) {
            case 'escape':
              store.dispatch({ type: 'palette.close' });
              return;
            case 'down':
              store.dispatch({ type: 'palette.move', delta: 1, count: rows.length });
              return;
            case 'up':
              store.dispatch({ type: 'palette.move', delta: -1, count: rows.length });
              return;
            case 'enter':
              rows[clamped]?.activate();
              return;
            default:
          }
        }}
      />
      {visible.map((row, i) => {
        const selected = start + i === clamped;
        return (
          <div
            key={row.key}
            testId={`palette-row-${row.key}`}
            onClick={row.activate}
            style={{
              flexDirection: 'row',
              alignItems: 'center',
              justifyContent: 'space-between',
              height: 28,
              paddingLeft: 10,
              paddingRight: 10,
              borderRadius: tokens.radius.tab,
              backgroundColor: selected ? tokens.bg.glassActive : 'transparent',
              borderWidth: tokens.border.width,
              borderColor: selected ? tokens.accent : 'transparent',
              cursor: 'pointer',
              hover: { backgroundColor: tokens.bg.glassHover },
            }}
          >
            <text style={{ color: tokens.fg.primary, fontSize: tokens.font.chrome }}>
              {row.title}
            </text>
            <text style={{ color: tokens.fg.muted, fontSize: tokens.font.chip }}>{row.hint}</text>
          </div>
        );
      })}
      {rows.length === 0 ? (
        <text
          testId="palette-empty"
          style={{ color: tokens.fg.muted, fontSize: tokens.font.chrome, padding: 8 }}
        >
          No matches
        </text>
      ) : null}
    </anchored>
  );

  function sessionRows(
    all: SessionView[],
    q: string,
    activeSessionId: number | null,
  ): Row[] {
    const matches = all
      .map((session) => ({ session, score: fuzzyScore(q, session.name) }))
      .filter((r): r is { session: SessionView; score: number } => r.score !== null)
      .sort((a, b) => b.score - a.score)
      .map(({ session }) => ({
        key: `session-${session.id}`,
        title: `${session.name}${session.id === activeSessionId ? ' ✓' : ''}`,
        hint: `${session.tabIds.length} tab${session.tabIds.length === 1 ? '' : 's'}`,
        activate: () => {
          store.dispatch({ type: 'palette.close' });
          void commandContext.client
            .request('session.set_active', { session: session.id })
            .catch(() => {
              store.dispatch({ type: 'toast.push', text: 'Could not switch session', kind: 'error' });
            });
        },
      }));

    const exact = all.some((s) => s.name.toLowerCase() === q.trim().toLowerCase());
    if (q.trim().length > 0 && !exact) {
      matches.push({
        key: 'session-new',
        title: `New Session “${q.trim()}”`,
        hint: registry.shortcutHint('session.new'),
        activate: () => {
          store.dispatch({ type: 'palette.close' });
          run('session.new', q.trim());
        },
      });
    }
    return matches;
  }
}

/** Exported for the palette-only tests in `05 §9` once a test renderer exists. */
export type { Command };
