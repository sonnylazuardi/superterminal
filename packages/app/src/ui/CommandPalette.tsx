/**
 * Command palette (05 §4): one `<anchored>` at top-center, 560 px wide, with an
 * `<input>` and a plain `<div>` list capped at 8 visible rows (nested scrolling
 * is unsupported, so the window of items shifts instead of scrolling).
 *
 * Placement: a Dialog opens at the top centre of the window (CONTEXT.md), so
 * the layer is given an explicit `position`. gpuix's `side`/`align` are
 * *trigger-relative* — the trigger here is the full-window root, so
 * `side="bottom"` meant "below the window" and the deferred layer took no part
 * in the centring flex: the palette landed bottom-left. With `position` the
 * anchor point is absolute and `anchor="topCenter"` centres the layer's top
 * edge on it.
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
  // Select the primitive, not a fresh object: `useSyncExternalStore` compares
  // selector results by identity and would re-render forever.
  const windowWidth = useWorkspace((s) => s.ui.window.width);
  const vertical = useWorkspace((s) => s.ui.verticalTabs);
  const placement = dialogPlacement(windowWidth, tokens, vertical);

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
      position={placement}
      style={{
        width: WIDTH,
        display: 'flex', // REQUIRED or children are blocks
        flexDirection: 'column',
        backgroundColor: tokens.bg.overlay,
        borderRadius: tokens.radius.panel,
        borderWidth: tokens.border.width,
        borderColor: tokens.border.glass,
        overflow: 'hidden', // keep rounded corners from being painted over
      }}
    >
      {/* `<anchored>` supports only click/enter/leave, so the outside-click
          listener lives on this inner div (see Menu.tsx). */}
      <div
        testId="palette-body"
        onMouseDownOutside={() => store.dispatch({ type: 'palette.close' })}
        style={{
          display: 'flex',
          flexDirection: 'column',
          padding: tokens.space.lg,
          gap: tokens.space.xs,
        }}
      >
        <input
          testId="palette-input"
          autoFocus
          value={query}
          placeholder={mode === 'commands' ? 'Run a command…' : 'Switch or create a session…'}
          style={{
            height: tokens.strip.paletteInputHeight,
            paddingLeft: tokens.space.lg,
            paddingRight: tokens.space.lg,
            marginBottom: tokens.space.md,
            borderRadius: tokens.radius.tab,
            backgroundColor: tokens.bg.glass,
            borderWidth: tokens.border.width,
            borderColor: tokens.accent,
            color: tokens.fg.primary,
            fontSize: tokens.font.paletteInput,
          }}
          onChange={(event) =>
            store.dispatch({
              type: 'palette.setQuery',
              query: String(event.value ?? ''),
            })
          }
          // Enter never reaches `onKeyDown`: the gpuix input binds it to its own
          // Submit action and GPUI consumes a keystroke that matched an action
          // before key listeners run. Esc/↑/↓ are unbound and do arrive.
          onSubmit={() => rows[clamped]?.activate()}
          onKeyDown={(event) => {
            switch (event.key) {
              case 'escape':
                store.dispatch({ type: 'palette.close' });
                return;
              case 'down':
                store.dispatch({
                  type: 'palette.move',
                  delta: 1,
                  count: rows.length,
                });
                return;
              case 'up':
                store.dispatch({
                  type: 'palette.move',
                  delta: -1,
                  count: rows.length,
                });
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
                display: 'flex',
                flexDirection: 'row',
                alignItems: 'center',
                height: tokens.strip.rowHeight,
                paddingLeft: tokens.space.lg,
                paddingRight: tokens.space.lg,
                gap: tokens.space.lg,
                borderRadius: tokens.radius.tab,
                backgroundColor: selected ? tokens.bg.glassActive : 'transparent',
                borderWidth: tokens.border.width,
                borderColor: selected ? tokens.accent : 'transparent',
                cursor: 'pointer',
                hover: { backgroundColor: tokens.bg.glassHover },
              }}
            >
              <text
                style={{
                  color: tokens.fg.primary,
                  fontSize: tokens.font.chrome,
                  flexGrow: 1,
                  minWidth: 0,
                  overflow: 'hidden',
                  whiteSpace: 'nowrap',
                  textOverflow: 'ellipsis',
                }}
              >
                {row.title}
              </text>
              <text
                style={{
                  color: tokens.fg.muted,
                  fontSize: tokens.font.chip,
                  flexShrink: 0,
                }}
              >
                {row.hint}
              </text>
            </div>
          );
        })}
        {rows.length === 0 ? (
          <div
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              height: tokens.strip.rowHeight,
            }}
          >
            <text
              testId="palette-empty"
              style={{ color: tokens.fg.muted, fontSize: tokens.font.chrome }}
            >
              No matches
            </text>
          </div>
        ) : null}
      </div>
    </anchored>
  );

  function sessionRows(all: SessionView[], q: string, activeSessionId: number | null): Row[] {
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
              store.dispatch({
                type: 'toast.push',
                text: 'Could not switch session',
                kind: 'error',
              });
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

/**
 * Where a Dialog's top-centre goes: horizontally centred, just below the
 * chrome — the title bar, plus the tab strip when it runs along the top.
 * Before the first size sample `width` is 0; anchoring at x=0 would put half
 * the panel off-screen (gpui's snap-to-window then shoves it to the left
 * margin), so fall back to a plausible centre until the sample lands.
 */
export function dialogPlacement(
  windowWidth: number,
  tokens: {
    strip: { titleBarHeight: number; height: number };
    space: { xl: number };
  },
  verticalTabs: boolean,
): { x: number; y: number } {
  const width = windowWidth > 0 ? windowWidth : 2 * WIDTH;
  const chrome = tokens.strip.titleBarHeight + (verticalTabs ? 0 : tokens.strip.height);
  return { x: Math.round(width / 2), y: chrome + tokens.space.xl };
}

/** Exported for the palette-only tests in `05 §9` once a test renderer exists. */
export type { Command };
