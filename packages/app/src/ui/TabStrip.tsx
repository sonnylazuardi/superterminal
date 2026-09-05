/**
 * Tab strip (05 §4, §7): session chip, one tab per Tab, new-tab button.
 *
 * In vertical mode this is ONLY the scrolling list — `App.tsx` owns the
 * sidebar column (width, background, divider). In horizontal mode it keeps
 * owning its own row chrome.
 *
 * Every `<text>` sets `color` explicitly — GPUI does not inherit it.
 */

import { useState } from 'react';
import { selectActiveSession, selectActiveTabId, selectActiveTabs } from '../state/selectors.js';
import { focusedSurfaceOf } from '../state/layout.js';
import type { SessionView, SurfaceView, TabView } from '../state/types.js';
import type { Tokens } from '../theme/tokens.js';
import { useRunCommand, useServices, useWorkspace } from './context.js';
import { ICONS, Icon } from './Icon.js';
import { displayTitle } from '../state/title.js';
import { debug } from '../util/debug.js';

const menuLog = debug('st:menu');

export function TabStrip() {
  const { tokens, store } = useServices();
  const vertical = useWorkspace((s) => s.ui.verticalTabs);
  const tabs = useWorkspace(selectActiveTabs);
  const activeTabId = useWorkspace(selectActiveTabId);
  const session = useWorkspace(selectActiveSession);
  const surfaces = useWorkspace((s) => s.surfaces);
  const focusedPaneByTab = useWorkspace((s) => s.ui.focusedPaneByTab);
  const confirmingCloseTabId = useWorkspace((s) => s.ui.confirmingCloseTabId);
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
        {tabs.map((tab) => (
          <Tab
            key={tab.id}
            tab={tab}
            surface={rowSurface(tab)}
            bell={anyBell(tab)}
            active={tab.id === activeTabId}
            confirming={confirmingCloseTabId === tab.id}
            vertical={vertical}
            tokens={tokens}
            onActivate={() => activate(tab)}
            onClose={() => run('tab.close', tab.id)}
            onAuxClick={(event) => openMenu(tab, event)}
          />
        ))}
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
        backgroundColor: tokens.bg.glass,
        borderColor: tokens.border.glass,
        borderBottomWidth: tokens.border.width,
        overflow: 'hidden',
      }}
    >
      <SessionChip session={session} tokens={tokens} vertical={vertical} />
      {tabs.map((tab) => (
        <Tab
          key={tab.id}
          tab={tab}
          surface={rowSurface(tab)}
          bell={anyBell(tab)}
          active={tab.id === activeTabId}
          confirming={confirmingCloseTabId === tab.id}
          vertical={vertical}
          tokens={tokens}
          onActivate={() => activate(tab)}
          onClose={() => run('tab.close', tab.id)}
          onAuxClick={(event) => openMenu(tab, event)}
        />
      ))}
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
    // A section header, not a pill. `display: 'flex'` is load-bearing:
    // without it `alignItems` is inert and the label sits at the TOP of the
    // 24pt band.
    return (
      <div
        testId="session-chip"
        onClick={() => run('session.switch')}
        style={{
          display: 'flex',
          alignItems: 'center',
          height: tokens.strip.sectionHeaderHeight,
          flexShrink: 0,
          paddingLeft: tokens.strip.rowPaddingX,
          paddingRight: tokens.strip.rowPaddingX,
          marginTop: tokens.space.sm,
          gap: tokens.space.sm,
          cursor: 'pointer',
        }}
      >
        <Icon name={ICONS.session} size={tokens.icon.section} color={tokens.fg.muted} />
        <text
          style={{
            color: tokens.fg.muted,
            fontSize: tokens.strip.sidebarSectionLabel,
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

export function Tab(props: {
  tab: TabView;
  /** The focused Pane's Surface — the one whose title the row shows. */
  surface: SurfaceView | undefined;
  /** Any Pane of the Tab rang. Defaults to the row Surface's own bell. */
  bell?: boolean;
  active: boolean;
  confirming: boolean;
  vertical: boolean;
  tokens: Tokens;
  onActivate: () => void;
  onClose: () => void;
  /** Right-click (gpuix `auxClick`): opens the tab Menu at the pointer. */
  onAuxClick?: (event: { x?: number; y?: number; isRightClick?: boolean; button?: number }) => void;
}) {
  const { tokens, surface } = props;
  const exited = surface?.status === 'exited';
  const title = displayTitle(surface?.title, 'shell');
  const badge = exited ? `⏻ ${surface?.exitSignal ?? surface?.exitCode ?? 0}` : null;
  const selected = props.active;
  const bell = props.bell ?? surface?.bell ?? false;
  const paneCount = props.tab.surfaceIds.length;

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
        borderColor: selected ? tokens.border.glass : 'transparent',
        backgroundColor: selected ? tokens.bg.glassActive : 'transparent',
        cursor: 'pointer',
        hover: { backgroundColor: selected ? tokens.bg.glassActive : tokens.bg.glassHover },
      }}
    >
      <div
        testId={`tab-${props.tab.id}-activate`}
        onClick={props.onActivate}
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
        hover: { backgroundColor: tokens.bg.glassHover },
      }}
    >
      <Icon name={ICONS.newTab} size={tokens.icon.button} color={tokens.fg.muted} />
    </div>
  );
}
