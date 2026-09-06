/**
 * Tab strip (05 §4, §7): session chip, one tab per Tab, new-tab button.
 *
 * In vertical mode this is ONLY the scrolling list — `App.tsx` owns the
 * sidebar column (width, background, divider). In horizontal mode it keeps
 * owning its own row chrome.
 *
 * Every `<text>` sets `color` explicitly — GPUI does not inherit it.
 *
 * # Reordering by drag
 *
 * `Tab` owns the pointer plumbing (press, threshold, delta, release); the
 * strip owns the geometry (`state/tab-drag.ts`) and the commit. Nothing is
 * sent until the release: `ui.tabDrag` is a local preview, and the release
 * fires one `tab.reorder`, only if the index actually changed. The row's
 * `onClick` still fires on release, so dragging a tab also activates it —
 * what a browser tab strip does.
 */

import { useRef, useState } from 'react';
import { selectActiveSession, selectActiveTabId, selectActiveTabs } from '../state/selectors.js';
import { focusedSurfaceOf } from '../state/layout.js';
import { REORDER_THRESHOLD, dropIndex, previewOrder, tabExtent } from '../state/tab-drag.js';
import type { SessionView, SurfaceView, TabView } from '../state/types.js';
import type { Tokens } from '../theme/tokens.js';
import { useRunCommand, useServices, useWorkspace } from './context.js';
import { ICONS, Icon } from './Icon.js';
import { displayTitle } from '../state/title.js';
import { debug } from '../util/debug.js';

const menuLog = debug('st:menu');
const dragLog = debug('st:tabdrag');

export function TabStrip() {
  const { tokens, store } = useServices();
  const vertical = useWorkspace((s) => s.ui.verticalTabs);
  const tabs = useWorkspace(selectActiveTabs);
  const activeTabId = useWorkspace(selectActiveTabId);
  const session = useWorkspace(selectActiveSession);
  const surfaces = useWorkspace((s) => s.surfaces);
  const focusedPaneByTab = useWorkspace((s) => s.ui.focusedPaneByTab);
  const confirmingCloseTabId = useWorkspace((s) => s.ui.confirmingCloseTabId);
  const tabDrag = useWorkspace((s) => s.ui.tabDrag);
  const run = useRunCommand();
  const { commandContext } = useServices();

  /** The row's Surface is the focused Pane's; bell lights for any Pane. */
  const rowSurface = (tab: TabView): SurfaceView | undefined =>
    surfaces[focusedSurfaceOf(tab, focusedPaneByTab[tab.id])];
  const anyBell = (tab: TabView): boolean => tab.surfaceIds.some((id) => surfaces[id]?.bell);
  const openMenu = (
    tab: TabView,
    event: { x?: number; y?: number; isRightClick?: boolean; button?: number },
  ) => {
    menuLog('auxClick tab', tab.id, 'right', event.isRightClick, 'at', event.x, event.y);
    if (!(event.isRightClick || event.button === 2)) return;
    store.dispatch({ type: 'menu.open', tabId: tab.id, x: event.x ?? 0, y: event.y ?? 0 });
  };

  const activate = (tab: TabView) => {
    void commandContext.client.request('tab.set_active', { tab: tab.id }).catch((err: unknown) => {
      store.dispatch({
        type: 'toast.push',
        text: `Could not switch tab: ${err instanceof Error ? err.message : String(err)}`,
        kind: 'error',
      });
    });
  };

  // The live preview: the dragged tab shown at the slot it is held over.
  const byId = new Map(tabs.map((tab) => [tab.id, tab]));
  const ordered = previewOrder(
    tabs.map((tab) => tab.id),
    tabDrag ? { tabId: tabDrag.tabId, to: tabDrag.to } : null,
  )
    .map((id) => byId.get(id))
    .filter((tab): tab is TabView => Boolean(tab));
  const extent = tabExtent(vertical, tokens);

  const onDragStart = (tab: TabView) => {
    const index = tabs.findIndex((t) => t.id === tab.id);
    if (index === -1) return;
    dragLog('begin', tab.id, 'from', index, 'extent', extent);
    store.dispatch({ type: 'tabDrag.begin', tabId: tab.id, index });
  };
  const onDrag = (tab: TabView, delta: number) => {
    const drag = store.getState().ui.tabDrag;
    if (!drag || drag.tabId !== tab.id) return;
    const count = store.getState().sessions[tab.sessionId]?.tabIds.length ?? tabs.length;
    store.dispatch({ type: 'tabDrag.to', index: dropIndex(drag.from, delta, extent, count) });
  };
  const onDragEnd = (tab: TabView) => {
    const drag = store.getState().ui.tabDrag;
    store.dispatch({ type: 'tabDrag.clear' });
    if (!drag || drag.tabId !== tab.id || drag.to === drag.from) return;
    dragLog('commit', tab.id, drag.from, '->', drag.to);
    void commandContext.client
      .request('tab.reorder', { tab: tab.id, index: drag.to })
      .catch((err: unknown) => {
        store.dispatch({
          type: 'toast.push',
          text: `Could not move tab: ${err instanceof Error ? err.message : String(err)}`,
          kind: 'error',
        });
      });
  };

  const rows = ordered.map((tab) => (
    <Tab
      key={tab.id}
      tab={tab}
      surface={rowSurface(tab)}
      bell={anyBell(tab)}
      active={tab.id === activeTabId}
      confirming={confirmingCloseTabId === tab.id}
      dragging={tabDrag?.tabId === tab.id}
      vertical={vertical}
      tokens={tokens}
      onActivate={() => activate(tab)}
      onClose={() => run('tab.close', tab.id)}
      onAuxClick={(event) => openMenu(tab, event)}
      onDragStart={() => onDragStart(tab)}
      onDrag={(delta) => onDrag(tab, delta)}
      onDragEnd={() => onDragEnd(tab)}
    />
  ));

  if (vertical) {
    return (
      <div
        testId="tab-strip"
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'stretch',
          // No `gap`: rows carry `marginBottom` and the two together doubled
          // the spacing.
          flexGrow: 1,
          minHeight: 0,
          overflowY: 'scroll',
          paddingLeft: tokens.strip.sidebarPadding,
          paddingRight: tokens.strip.sidebarPadding,
          paddingBottom: tokens.strip.sidebarPadding,
        }}
      >
        <SessionChip session={session} tokens={tokens} vertical={vertical} />
        {rows}
      </div>
    );
  }

  return (
    <div
      testId="tab-strip"
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        gap: tokens.strip.gap,
        height: tokens.strip.height,
        paddingLeft: tokens.strip.paddingX,
        paddingRight: tokens.strip.paddingX,
        backgroundColor: tokens.bg.chrome,
        borderColor: tokens.border.glass,
        borderBottomWidth: tokens.border.width,
        overflow: 'hidden',
      }}
    >
      <SessionChip session={session} tokens={tokens} vertical={vertical} />
      {rows}
      <NewTabButton tokens={tokens} onClick={() => run('tab.new')} />
    </div>
  );
}

export function SessionChip(props: {
  session: SessionView | null;
  tokens: Tokens;
  vertical: boolean;
}) {
  const { tokens } = props;
  const { store, commandContext } = useServices();
  const renamingId = useWorkspace((s) => s.ui.renamingSessionId);
  const [draft, setDraft] = useState<string | null>(null);
  const run = useRunCommand();

  if (!props.session) return null;
  const session = props.session;
  const renaming = renamingId === session.id;

  const commit = (name: string) => {
    store.dispatch({ type: 'session.endRename' });
    setDraft(null);
    const trimmed = name.trim();
    if (trimmed.length === 0 || trimmed === session.name) return; // empty names rejected client-side
    void commandContext.client
      .request('session.rename', { session: session.id, name: trimmed })
      .catch(() => {
        store.dispatch({ type: 'toast.push', text: 'Rename failed', kind: 'error' });
      });
  };

  if (renaming) {
    // The input is untouched by `userSelect`: it is a different element type
    // with its own focus handle and editing, and selecting text inside it
    // must keep working.
    return (
      <input
        testId="session-rename-input"
        autoFocus
        value={draft ?? session.name}
        style={{
          width: props.vertical ? '100%' : tokens.strip.renameWidth,
          height: tokens.strip.chipHeight,
          paddingLeft: tokens.space.lg,
          paddingRight: tokens.space.lg,
          borderRadius: tokens.radius.chip,
          backgroundColor: tokens.bg.glassActive,
          borderWidth: tokens.border.width,
          borderColor: tokens.accent,
          color: tokens.fg.primary,
          fontSize: tokens.font.chip,
        }}
        onChange={(event) => setDraft(String(event.value ?? ''))}
        onSubmit={() => commit(draft ?? session.name)}
        onKeyDown={(event) => {
          if (event.key === 'escape') {
            store.dispatch({ type: 'session.endRename' });
            setDraft(null);
          }
        }}
      />
    );
  }

  if (props.vertical) {
    // The Session header is laid out exactly like a tab row — same height,
    // paddings, icon box and font — so the person icon sits on the chevron
    // column and the name reads at the same size as the tab titles. It is a
    // button (opens the Session switcher), so it also gets the row hover.
    // `display: 'flex'` is load-bearing: without it `alignItems` is inert.
    return (
      <div
        testId="session-chip"
        onClick={() => run('session.switch')}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          height: tokens.strip.rowHeight,
          flexShrink: 0,
          paddingLeft: tokens.strip.rowPaddingX,
          paddingRight: tokens.strip.rowPaddingX,
          marginTop: tokens.space.sm,
          marginBottom: tokens.space.xs,
          gap: tokens.space.sm,
          borderRadius: tokens.radius.tab,
          cursor: 'pointer',
          userSelect: 'none',
          hover: { backgroundColor: tokens.bg.glassHover },
        }}
      >
        <div
          style={{
            width: tokens.strip.rowIcon,
            height: tokens.strip.rowIcon,
            flexShrink: 0,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
          }}
        >
          <Icon name={ICONS.session} size={tokens.icon.row} color={tokens.fg.muted} />
        </div>
        <text
          style={{
            color: tokens.fg.muted,
            fontSize: tokens.font.chrome,
            flexGrow: 1,
            minWidth: 0,
            overflow: 'hidden',
            whiteSpace: 'nowrap',
            textOverflow: 'ellipsis',
          }}
        >
          {session.name}
        </text>
      </div>
    );
  }

  return (
    <div
      testId="session-chip"
      onClick={() => run('session.switch')}
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        height: tokens.strip.chipHeight,
        flexShrink: 0,
        paddingLeft: tokens.space.xl,
        paddingRight: tokens.space.xl,
        marginRight: tokens.space.sm,
        borderRadius: tokens.radius.chip,
        backgroundColor: tokens.bg.glass,
        cursor: 'pointer',
        userSelect: 'none',
        hover: { backgroundColor: tokens.bg.glassHover },
      }}
    >
      <Icon name={ICONS.session} size={tokens.icon.chip} color={tokens.fg.muted} />
      <text style={{ color: tokens.fg.muted, fontSize: tokens.font.chip, marginLeft: tokens.space.sm }}>
        {session.name}
      </text>
    </div>
  );
}

/** The mouse payload fields a tab reads; gpuix's `EventPayload` is a superset. */
interface TabMouseEvent {
  x?: number;
  y?: number;
  button?: number;
  /** gpuix sets this on `mouseMove`; a released primary button ends the drag. */
  pressedButton?: number;
}

const PRIMARY = 0;

export function Tab(props: {
  tab: TabView;
  /** The focused Pane's Surface — the one whose title the row shows. */
  surface: SurfaceView | undefined;
  /** Any Pane of the Tab rang. Defaults to the row Surface's own bell. */
  bell?: boolean;
  active: boolean;
  confirming: boolean;
  /** This row is the one being dragged; it is outlined in accent. */
  dragging?: boolean;
  vertical: boolean;
  tokens: Tokens;
  onActivate: () => void;
  onClose: () => void;
  /** Right-click (gpuix `auxClick`): opens the tab Menu at the pointer. */
  onAuxClick?: (event: { x?: number; y?: number; isRightClick?: boolean; button?: number }) => void;
  /** The press moved past `REORDER_THRESHOLD`; a drag is now in progress. */
  onDragStart?: () => void;
  /** Pointer travel since the press, px along the strip's axis. */
  onDrag?: (delta: number) => void;
  /** The button came up after a drag (never after a plain click). */
  onDragEnd?: () => void;
}) {
  const { tokens, surface } = props;
  const exited = surface?.status === 'exited';
  const title = displayTitle(surface?.title, 'shell');
  const badge = exited ? `⏻ ${surface?.exitSignal ?? surface?.exitCode ?? 0}` : null;
  const selected = props.active;
  const bell = props.bell ?? surface?.bell ?? false;
  const paneCount = props.tab.surfaceIds.length;

  // The press, while the button is down. Not state: a re-render per pointer
  // move would be wasted, the store already re-renders the strip on `to`.
  const press = useRef<{ pos: number; dragging: boolean } | null>(null);
  const axisPos = (event: TabMouseEvent): number | null => {
    const pos = props.vertical ? event.y : event.x;
    return typeof pos === 'number' && Number.isFinite(pos) ? pos : null;
  };
  const finish = () => {
    const current = press.current;
    press.current = null;
    if (current?.dragging) props.onDragEnd?.();
  };
  const onMouseDown = (event: TabMouseEvent) => {
    if (event.button !== undefined && event.button !== PRIMARY) return;
    const pos = axisPos(event);
    if (pos === null) return;
    press.current = { pos, dragging: false };
  };
  const onMouseMove = (event: TabMouseEvent) => {
    const current = press.current;
    if (!current) return;
    if (event.pressedButton !== undefined && event.pressedButton !== PRIMARY) {
      // The button went up somewhere we did not see (focus change, etc.).
      finish();
      return;
    }
    const pos = axisPos(event);
    if (pos === null) return;
    const delta = pos - current.pos;
    if (!current.dragging) {
      // Gate on the threshold so a plain click still just activates.
      if (Math.abs(delta) < REORDER_THRESHOLD) return;
      current.dragging = true;
      props.onDragStart?.();
    }
    props.onDrag?.(delta);
  };

  return (
    // LAYOUT ONLY — no onClick here. gpuix fires an ancestor's onClick as
    // well as the child's, so a nested × would close the tab AND activate
    // the deleted id (`tab N does not exist`). Activate and close are
    // siblings instead.
    <div
      testId={`tab-${props.tab.id}`}
      // A right-click is not a click: `onAuxClick` here does not fire the
      // children's `onClick`, so this is safe on the layout row.
      {...(props.onAuxClick ? { onAuxClick: props.onAuxClick } : {})}
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        ...(props.vertical
          ? {
              height: tokens.strip.rowHeight,
              width: '100%',
              flexShrink: 0,
              marginBottom: tokens.space.xs,
              paddingLeft: tokens.strip.rowPaddingX,
              paddingRight: tokens.strip.rowPaddingX,
            }
          : {
              height: tokens.strip.tabHeight,
              minWidth: tokens.strip.tabMinWidth,
              maxWidth: tokens.strip.tabMaxWidth,
              flexShrink: 1,
              paddingLeft: tokens.space.md,
              paddingRight: tokens.space.lg,
            }),
        gap: tokens.space.sm,
        overflow: 'hidden',
        borderRadius: tokens.radius.tab,
        // Always a 1px border (transparent when idle) so rows do not shift
        // 1px sideways on activate.
        borderWidth: tokens.border.width,
        borderColor: props.dragging
          ? tokens.accent
          : selected
            ? tokens.border.glass
            : 'transparent',
        backgroundColor: selected ? tokens.bg.glassActive : 'transparent',
        cursor: 'pointer',
        // Pressing a tab must not start a text selection, and dragging it to
        // reorder must not paint the title blue. `userSelect` inherits to the
        // title and every badge inside.
        userSelect: 'none',
        hover: { backgroundColor: selected ? tokens.bg.glassActive : tokens.bg.glassHover },
      }}
    >
      <div
        testId={`tab-${props.tab.id}-activate`}
        onClick={props.onActivate}
        // The press is on the activate child, not the row: the close × is
        // its sibling, and starting a drag from it would fight the button.
        // `onMouseDown` + `onMouseMove` on the same element give it GPUI
        // pointer capture, so moves and the release keep arriving after the
        // pointer leaves the row (`ui/drag.ts` explains why an ancestor
        // cannot do this).
        onMouseDown={onMouseDown}
        onMouseMove={onMouseMove}
        onMouseUp={finish}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          flexGrow: 1,
          // Repeat minWidth/overflow here: this is now the flex item the
          // title shrinks inside.
          minWidth: 0,
          overflow: 'hidden',
          gap: tokens.space.sm,
        }}
      >
        {bell ? (
          <text
            style={{
              color: tokens.accent,
              fontSize: tokens.font.chrome,
              flexShrink: 0,
            }}
          >
            ●
          </text>
        ) : null}
        {props.vertical ? (
          <div
            style={{
              width: tokens.strip.rowIcon,
              height: tokens.strip.rowIcon,
              flexShrink: 0,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              borderRadius: tokens.radius.chipSmall,
              backgroundColor: selected ? tokens.bg.glassActive : tokens.bg.glassSubtle,
            }}
          >
            <Icon
              name={ICONS.chevron}
              size={tokens.icon.chip}
              color={selected ? tokens.accent : tokens.fg.muted}
            />
          </div>
        ) : null}
        <text
          style={{
            color: exited ? tokens.fg.muted : tokens.fg.primary,
            fontSize: tokens.font.chrome,
            flexGrow: 1,
            minWidth: 0,
            overflow: 'hidden',
            whiteSpace: 'nowrap',
            textOverflow: 'ellipsis',
          }}
        >
          {title}
        </text>
        {paneCount > 1 ? (
          // Pane count: a split icon and the number.
          <div
            testId={`tab-${props.tab.id}-panes`}
            style={{ display: 'flex', alignItems: 'center', gap: tokens.space.xs, flexShrink: 0 }}
          >
            <Icon name={ICONS.panes} size={tokens.icon.chip} color={tokens.fg.muted} />
            <text style={{ color: tokens.fg.muted, fontSize: tokens.font.chip }}>{String(paneCount)}</text>
          </div>
        ) : null}
        {badge ? (
          <text
            testId={`tab-${props.tab.id}-exited`}
            style={{ color: tokens.fg.muted, fontSize: tokens.font.chip, flexShrink: 0 }}
          >
            {badge}
          </text>
        ) : null}
        {props.confirming ? (
          <text
            testId={`tab-${props.tab.id}-confirm`}
            style={{ color: tokens.fg.danger, fontSize: tokens.font.chip, flexShrink: 0 }}
          >
            Close?
          </text>
        ) : null}
      </div>
      <div
        testId={`tab-${props.tab.id}-close`}
        onClick={props.onClose}
        style={{
          width: tokens.strip.rowIcon,
          height: tokens.strip.rowIcon,
          flexShrink: 0,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          borderRadius: tokens.radius.chipSmall,
          cursor: 'pointer',
          hover: { backgroundColor: tokens.bg.glassHover },
        }}
      >
        <Icon name={ICONS.close} size={tokens.icon.chip} color={tokens.fg.muted} />
      </div>
    </div>
  );
}

export function NewTabButton(props: { tokens: Tokens; onClick: () => void }) {
  const { tokens } = props;
  return (
    <div
      testId="new-tab"
      onClick={props.onClick}
      style={{
        width: tokens.strip.iconButton,
        height: tokens.strip.iconButton,
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        borderRadius: tokens.radius.tab,
        cursor: 'pointer',
        userSelect: 'none',
        hover: { backgroundColor: tokens.bg.glassHover },
      }}
    >
      <Icon name={ICONS.newTab} size={tokens.icon.button} color={tokens.fg.muted} />
    </div>
  );
}
