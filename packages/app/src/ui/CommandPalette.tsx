/**
 * The command palette (05 §4, 08): one `<anchored>` at top-center, 560 px
 * wide, with an `<input>` and a plain `<div>` list capped at 8 visible rows
 * (nested scrolling is unsupported, so the window of items shifts instead of
 * scrolling).
 *
 * Placement: a Dialog opens at the top centre of the window (CONTEXT.md), so
 * the layer is given an explicit `position`. gpuix's `side`/`align` are
 * *trigger-relative* — the trigger here is the full-window root, so
 * `side="bottom"` meant "below the window" and the deferred layer took no part
 * in the centring flex: the palette landed bottom-left. With `position` the
 * anchor point is absolute and `anchor="topCenter"` centres the layer's top
 * edge on it.
 *
 * Rows (08 Q1): commands, tabs and sessions in one list (`all`, ⌘K / Ctrl+K),
 * or sessions only (`sessions`, Switch Session…). Local fuzzy matching is
 * instant; when a Jev key is configured the settled query is also ranked by
 * Jev and may promote or fill rows (`ai/palette-rank.ts`). A ✦ marks a row Jev
 * placed. In sessions mode a trailing row offers **New Session "‹query›"**
 * when the query matches no existing name.
 *
 * Opening the palette also fetches what each Tab is showing
 * (`use-screen-text.ts`), so a tab can be found by a word that is only on its
 * screen; the rows re-rank when that lands and are today's rows until it does.
 */

import { mergeRanking } from '../ai/palette-rank.js';
import { useFullWorkspace, useRunCommand, useServices, useWorkspace } from './context.js';
import { aiOffRow, buildRows, type PaletteRow } from './palette-rows.js';
import { usePaletteRanking } from './use-palette-ranking.js';
import { useScreenText } from './use-screen-text.js';

const WIDTH = 560;
const MAX_ROWS = 8;

export function CommandPalette() {
  const { tokens, registry, store, commandContext, ai } = useServices();
  const state = useFullWorkspace();
  const open = state.ui.paletteOpen;
  const mode = state.ui.paletteMode;
  const query = state.ui.paletteQuery;
  const index = state.ui.paletteIndex;
  const run = useRunCommand();
  // Select the primitive, not a fresh object: `useSyncExternalStore` compares
  // selector results by identity and would re-render forever.
  const windowWidth = useWorkspace((s) => s.ui.window.width);
  const vertical = useWorkspace((s) => s.ui.verticalTabs);
  const aiStatus = useWorkspace((s) => s.ui.ai.status);
  const aiEnabled = useWorkspace((s) => s.ui.ai.enabled);
  const screenContext = useWorkspace((s) => s.ui.ai.screenContext);
  const placement = dialogPlacement(windowWidth, tokens, vertical);

  // Fetched once per opening; an empty map is exactly the old palette (08 §H).
  const screenText = useScreenText({ client: commandContext.client, open, state });

  const built = buildRows({
    state,
    commands: registry.commands,
    mode,
    query,
    shortcutHint: (id) => registry.shortcutHint(id),
    screenText: screenText.screens,
    screenContext,
  });

  const ranking = usePaletteRanking({
    ai: mode === 'all' ? ai : null,
    store,
    open,
    query,
    candidates: built.candidates,
    activeTabLabel: built.activeTabLabel,
  });

  if (!open) return null;

  const trimmed = query.trim();
  const merged = mergeRanking({
    local: built.local,
    byKey: built.all,
    ranking: ranking.forQuery === trimmed ? ranking.ranking : null,
    selectedIndex: index,
  });
  let rows: PaletteRow[] = merged.rows;
  // The empty state teaches the one setup step (08 Q17).
  const offerSetup = rows.length === 0 && mode === 'all' && trimmed.length > 0 && aiEnabled && aiStatus === 'off';
  if (offerSetup) rows = [aiOffRow()];

  const clamped = Math.min(index, Math.max(0, rows.length - 1));
  // No inner scroll container: shift the window of items instead.
  const start = Math.max(0, Math.min(clamped - MAX_ROWS + 1, rows.length - MAX_ROWS));
  const visible = rows.slice(Math.max(0, start), Math.max(0, start) + MAX_ROWS);

  const close = () => store.dispatch({ type: 'palette.close' });

  const activate = (row: PaletteRow) => {
    const target = row.target;
    switch (target.type) {
      case 'command':
        close();
        run(target.id);
        return;
      case 'tab':
        close();
        void commandContext.client.request('tab.set_active', { tab: target.id }).catch(() => {
          store.dispatch({ type: 'toast.push', text: 'Could not switch tab', kind: 'error' });
        });
        return;
      case 'session':
        close();
        void commandContext.client.request('session.set_active', { session: target.id }).catch(() => {
          store.dispatch({ type: 'toast.push', text: 'Could not switch session', kind: 'error' });
        });
        return;
      case 'session.new':
        close();
        run('session.new', target.name);
        return;
      case 'ai.settings':
        run('ai.settings');
        return;
      default:
        return;
    }
  };

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
        onMouseDownOutside={close}
        style={{
          display: 'flex',
          flexDirection: 'column',
          padding: tokens.space.lg,
          gap: tokens.space.xs,
        }}
      >
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', marginBottom: tokens.space.md }}>
          <input
            testId="palette-input"
            autoFocus
            value={query}
            placeholder={mode === 'all' ? 'Run a command, jump to a tab…' : 'Switch or create a session…'}
            style={{
              flexGrow: 1,
              height: tokens.strip.paletteInputHeight,
              paddingLeft: tokens.space.lg,
              paddingRight: tokens.space.lg,
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
            onSubmit={() => {
              const row = rows[clamped];
              if (row) activate(row);
            }}
            onKeyDown={(event) => {
              switch (event.key) {
                case 'escape':
                  close();
                  return;
                case 'down':
                  store.dispatch({ type: 'palette.move', delta: 1, count: rows.length });
                  return;
                case 'up':
                  store.dispatch({ type: 'palette.move', delta: -1, count: rows.length });
                  return;
                default:
              }
            }}
          />
          {/* In-flight dot (08 Q14): the only visible "thinking" state. */}
          {ranking.inFlight ? (
            <text
              testId="palette-thinking"
              style={{ color: tokens.fg.muted, fontSize: tokens.font.chip, paddingLeft: tokens.space.md }}
            >
              ✦
            </text>
          ) : null}
        </div>
        {visible.map((row, i) => {
          const selected = start + i === clamped;
          const byJev = merged.promoted.has(row.key);
          return (
            <div
              key={row.key}
              testId={`palette-row-${row.key}`}
              onClick={() => activate(row)}
              style={{
                // Chrome text is not prose: a press here must not start a text
                // selection (gpuix `<text>` is selectable by default; this inherits).
                userSelect: 'none',
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
                  color: byJev ? tokens.accent : tokens.fg.muted,
                  fontSize: tokens.font.chip,
                  flexShrink: 0,
                }}
              >
                {byJev ? `✦ ${row.hint}` : row.hint}
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
              {ranking.inFlight ? 'Asking Jev…' : 'No matches'}
            </text>
          </div>
        ) : null}
      </div>
    </anchored>
  );
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
export type { Command } from '../commands/registry.js';
