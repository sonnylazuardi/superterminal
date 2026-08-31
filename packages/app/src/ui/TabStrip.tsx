/**
 * Tab strip (05 §4, §7): session chip, one tab per Tab, new-tab button.
 * Horizontal (height 36) and vertical (width 220) variants share every child.
 *
 * Every `<text>` sets `color` explicitly — GPUI does not inherit it.
 */

import { useState } from 'react';
import { selectActiveSession, selectActiveTabId, selectActiveTabs } from '../state/selectors.js';
import type { SessionView, SurfaceView, TabView } from '../state/types.js';
import type { Tokens } from '../theme/tokens.js';
import { useRunCommand, useServices, useWorkspace } from './context.js';

export function TabStrip() {
  const { tokens, store } = useServices();
  const vertical = useWorkspace((s) => s.ui.verticalTabs);
  const tabs = useWorkspace(selectActiveTabs);
  const activeTabId = useWorkspace(selectActiveTabId);
  const session = useWorkspace(selectActiveSession);
  const surfaces = useWorkspace((s) => s.surfaces);
  const confirmingCloseTabId = useWorkspace((s) => s.ui.confirmingCloseTabId);
  const run = useRunCommand();
  const { commandContext } = useServices();

  const activate = (tab: TabView) => {
    void commandContext.client.request('tab.set_active', { tab: tab.id }).catch(() => {
      store.dispatch({ type: 'toast.push', text: 'Could not switch tab', kind: 'error' });
    });
  };

  return (
    <div
      testId="tab-strip"
      style={{
        display: 'flex',
        flexDirection: vertical ? 'column' : 'row',
        alignItems: vertical ? 'stretch' : 'center',
        gap: tokens.strip.gap,
        ...(vertical
          ? { width: tokens.strip.verticalWidth, height: '100%', padding: tokens.strip.paddingX }
          : {
              height: tokens.strip.height,
              paddingLeft: tokens.strip.paddingX,
              paddingRight: tokens.strip.paddingX,
            }),
        backgroundColor: tokens.bg.glass,
        borderColor: tokens.border.glass,
        ...(vertical
          ? { borderRightWidth: tokens.border.width }
          : { borderBottomWidth: tokens.border.width }),
      }}
    >
      <SessionChip session={session} tokens={tokens} vertical={vertical} />
      {tabs.map((tab) => (
        <Tab
          key={tab.id}
          tab={tab}
          surface={surfaces[tab.surfaceId]}
          active={tab.id === activeTabId}
          confirming={confirmingCloseTabId === tab.id}
          vertical={vertical}
          tokens={tokens}
          onActivate={() => activate(tab)}
          onClose={() => run('tab.close', tab.id)}
        />
      ))}
      <NewTabButton tokens={tokens} vertical={vertical} onClick={() => run('tab.new')} />
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
          width: 140,
          height: 22,
          paddingLeft: 8,
          paddingRight: 8,
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

  return (
    <div
      testId="session-chip"
      onClick={() => run('session.switch')}
      style={{
        height: 22,
        paddingLeft: 10,
        paddingRight: 10,
        borderRadius: tokens.radius.chip,
        backgroundColor: tokens.bg.glass,
        alignItems: 'center',
        justifyContent: 'center',
        cursor: 'pointer',
        hover: { backgroundColor: tokens.bg.glassHover },
        ...(props.vertical ? { marginBottom: 6 } : { marginRight: 4 }),
      }}
    >
      <text style={{ color: tokens.fg.muted, fontSize: tokens.font.chip }}>{session.name}</text>
    </div>
  );
}

export function Tab(props: {
  tab: TabView;
  surface: SurfaceView | undefined;
  active: boolean;
  confirming: boolean;
  vertical: boolean;
  tokens: Tokens;
  onActivate: () => void;
  onClose: () => void;
}) {
  const { tokens, surface } = props;
  const exited = surface?.status === 'exited';
  const title = surface?.title ?? 'shell';
  const badge = exited ? `⏻ ${surface?.exitSignal ?? surface?.exitCode ?? 0}` : null;

  return (
    <div
      testId={`tab-${props.tab.id}`}
      onClick={props.onActivate}
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
        ...(props.vertical
          ? { minHeight: tokens.strip.tabHeight, width: '100%' }
          : { height: tokens.strip.tabHeight, maxWidth: tokens.strip.tabMaxWidth }),
        paddingLeft: 10,
        paddingRight: 8,
        borderRadius: tokens.radius.tab,
        backgroundColor: props.active ? tokens.bg.glassActive : 'transparent',
        cursor: 'pointer',
        hover: { backgroundColor: props.active ? tokens.bg.glassActive : tokens.bg.glassHover },
      }}
    >
      {surface?.bell ? (
        <text style={{ color: tokens.accent, fontSize: tokens.font.chrome }}>●</text>
      ) : null}
      <text
        style={{
          color: exited ? tokens.fg.muted : tokens.fg.primary,
          fontSize: tokens.font.chrome,
          flexGrow: 1,
          whiteSpace: props.vertical ? 'normal' : 'nowrap',
          textOverflow: 'ellipsis',
          ...(props.vertical ? { lineClamp: 2 } : {}),
        }}
      >
        {title}
      </text>
      {badge ? (
        <text testId={`tab-${props.tab.id}-exited`} style={{ color: tokens.fg.muted, fontSize: tokens.font.chip }}>
          {badge}
        </text>
      ) : null}
      {props.confirming ? (
        <text
          testId={`tab-${props.tab.id}-confirm`}
          style={{ color: tokens.fg.danger, fontSize: tokens.font.chip }}
        >
          Close?
        </text>
      ) : null}
      <div
        testId={`tab-${props.tab.id}-close`}
        onClick={props.onClose}
        style={{
          width: 16,
          height: 16,
          alignItems: 'center',
          justifyContent: 'center',
          borderRadius: 4,
          cursor: 'pointer',
          hover: { backgroundColor: tokens.bg.glassHover },
        }}
      >
        <text style={{ color: tokens.fg.muted, fontSize: tokens.font.chip }}>✕</text>
      </div>
    </div>
  );
}

export function NewTabButton(props: { tokens: Tokens; vertical: boolean; onClick: () => void }) {
  const { tokens } = props;
  return (
    <div
      testId="new-tab"
      onClick={props.onClick}
      style={{
        width: props.vertical ? '100%' : 24,
        height: 24,
        alignItems: 'center',
        justifyContent: 'center',
        borderRadius: tokens.radius.tab,
        cursor: 'pointer',
        hover: { backgroundColor: tokens.bg.glassHover },
      }}
    >
      <text style={{ color: tokens.fg.muted, fontSize: tokens.font.chrome }}>＋</text>
    </div>
  );
}
