/**
 * Root component (05 §4).
 *
 *   <App vertical>              <App horizontal>
 *    ├─ <Banner/>                ├─ <TitleBar/>
 *    ├─ <Frame row>              ├─ <Banner/>
 *    │   ├─ sidebar              ├─ <Frame column>
 *    │   │   ├─ <SidebarHeader/> │   ├─ <TabStrip/>
 *    │   │   ├─ <TabStrip/>      │   └─ <SurfaceHost/>
 *    │   │   └─ <SidebarFooter/> ├─ <CommandPalette/>
 *    │   ├─ divider              └─ <StatusToasts/>
 *    │   └─ content
 *    │       ├─ <ContentHeader/>
 *    │       └─ <SurfaceHost/>
 *    ├─ <CommandPalette/>
 *    └─ <StatusToasts/>
 *
 * Exactly one `<terminal-grid>` is mounted: the visible tab's (Q44).
 * There is no full-width title bar and no `TitleBarSpacer` in vertical mode:
 * the sidebar column IS the traffic-light row.
 *
 * The root `onKeyDown` receives the keystrokes `<terminal-grid>` declined
 * (`passthroughKeys`, 05 §5) and runs the first enabled matching command.
 */

import type { KeyEventLike } from '../platform/keys.js';
import { selectActiveSurface } from '../state/selectors.js';
import { debug } from '../util/debug.js';
import { Banner, StatusToasts } from './Banner.js';
import { CommandPalette } from './CommandPalette.js';
import { AppProvider, useRunCommand, useServices, useWorkspace, type AppServices } from './context.js';
import { ICONS, IconButton, sidebarIconInset } from './Icon.js';
import { SurfaceHost } from './SurfaceHost.js';
import { TabStrip } from './TabStrip.js';

const keysLog = debug('st:keys');

export function App(props: { services: AppServices }) {
  return (
    <AppProvider services={props.services}>
      <AppFrame />
    </AppProvider>
  );
}

function AppFrame() {
  const { registry, commandContext, store, tokens } = useServices();
  const vertical = useWorkspace((s) => s.ui.verticalTabs);
  const paletteOpen = useWorkspace((s) => s.ui.paletteOpen);

  const onKeyDown = (event: KeyEventLike) => {
    // While the palette is open it owns Esc/↑/↓/Enter; the `<input>` handles
    // them, so only bail out for those keys (⌘K still toggles session mode).
    if (paletteOpen && ['escape', 'up', 'down', 'enter'].includes(event.key ?? '')) return;

    const match = registry.matchKeybinding(event, store.getState());
    keysLog(
      'key=%s matched=%s',
      event.key ?? '(none)',
      match ? match.command.id : '(none)',
    );
    if (!match) return;
    void Promise.resolve(match.command.run(commandContext, match.arg)).catch((err: unknown) => {
      store.dispatch({
        type: 'toast.push',
        text: err instanceof Error ? err.message : String(err),
        kind: 'error',
      });
    });
  };

  return (
    <div
      testId="app-root"
      onKeyDown={onKeyDown}
      style={{
        display: 'flex',
        flexDirection: 'column',
        width: '100%',
        height: '100%',
        backgroundColor: 'transparent',
      }}
    >
      {vertical ? null : <TitleBar />}
      <Banner />
      {vertical ? (
        <div testId="frame" style={{ display: 'flex', flexDirection: 'row', flexGrow: 1, overflow: 'hidden' }}>
          <div
            testId="sidebar"
            style={{
              display: 'flex',
              flexDirection: 'column',
              width: tokens.strip.verticalWidth,
              flexShrink: 0,
              backgroundColor: tokens.bg.glass,
            }}
          >
            <SidebarHeader />
            <TabStrip />
            <SidebarFooter />
          </div>
          <div
            testId="sidebar-divider"
            style={{ width: tokens.border.width, flexShrink: 0, backgroundColor: tokens.border.glass }}
          />
          <div
            testId="content"
            style={{
              display: 'flex',
              flexDirection: 'column',
              flexGrow: 1,
              minWidth: 0,
              overflow: 'hidden',
              // Opaque windows (Windows, X11) paint white where no element
              // does: the content column needs its own surface, like the
              // sidebar has. On blurred macOS this is the same glass.
              backgroundColor: tokens.bg.glass,
            }}
          >
            <ContentHeader />
            <SurfaceHost />
          </div>
        </div>
      ) : (
        <div
          testId="frame"
          style={{ display: 'flex', flexDirection: 'column', flexGrow: 1, overflow: 'hidden' }}
        >
          <TabStrip />
          <SurfaceHost />
        </div>
      )}
      <CommandPalette />
      <StatusToasts />
    </div>
  );
}

function SidebarHeader() {
  const { tokens, platform } = useServices();
  const run = useRunCommand();
  const inset = sidebarIconInset(tokens);
  return (
    <div
      testId="sidebar-header"
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        flexShrink: 0,
        height: tokens.strip.titleBarHeight,
        gap: tokens.space.xs,
        paddingLeft: platform.isMac ? tokens.padding.trafficLights : inset,
        paddingRight: inset,
      }}
    >
      <IconButton
        testId="sidebar-toggle"
        glyph={ICONS.sidebar}
        tokens={tokens}
        onClick={() => run('view.toggleVerticalTabs')}
      />
      <div style={{ flexGrow: 1 }} />
      <IconButton
        testId="new-tab"
        glyph={ICONS.newTab}
        tokens={tokens}
        onClick={() => run('tab.new')}
      />
    </div>
  );
}

function SidebarFooter() {
  const { tokens } = useServices();
  const run = useRunCommand();
  const inset = sidebarIconInset(tokens);
  return (
    <div
      testId="sidebar-footer"
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        flexShrink: 0,
        height: tokens.strip.footerHeight,
        paddingLeft: inset,
        paddingRight: inset,
        borderTopWidth: tokens.border.width,
        borderColor: tokens.border.glass,
      }}
    >
      <IconButton
        testId="sidebar-palette"
        glyph={ICONS.palette}
        tokens={tokens}
        onClick={() => run('palette.commands')}
      />
      <div style={{ flexGrow: 1 }} />
      <IconButton
        testId="sidebar-new-session"
        glyph={ICONS.newSession}
        tokens={tokens}
        onClick={() => run('session.new')}
      />
    </div>
  );
}

function ContentHeader() {
  const { tokens } = useServices();
  const surface = useWorkspace(selectActiveSurface);
  const title = surface?.title && surface.title.length > 0 ? surface.title : 'superterminal';
  return (
    <div
      testId="content-header"
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        flexShrink: 0,
        height: tokens.strip.titleBarHeight,
        paddingLeft: tokens.strip.rowPaddingX,
        paddingRight: tokens.strip.rowPaddingX,
        // Same opaque-window reason as the content column: without its own
        // fill this strip shows the window's white behind the title.
        backgroundColor: tokens.bg.glass,
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
        <text style={{ color: tokens.fg.muted, fontSize: tokens.font.chip }}>{ICONS.surface}</text>
      </div>
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
        {title}
      </text>
    </div>
  );
}

function TitleBar() {
  const { tokens, platform } = useServices();
  const run = useRunCommand();
  const surface = useWorkspace(selectActiveSurface);
  const title = surface?.title && surface.title.length > 0 ? surface.title : 'superterminal';
  return (
    <div
      testId="titlebar"
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        flexShrink: 0,
        height: tokens.strip.titleBarHeight,
        gap: tokens.space.xs,
        paddingLeft: platform.isMac ? tokens.padding.trafficLights : tokens.strip.sidebarPadding,
        paddingRight: tokens.strip.sidebarPadding,
        backgroundColor: tokens.bg.glass,
        borderBottomWidth: tokens.border.width,
        borderColor: tokens.border.glass,
      }}
    >
      <IconButton
        testId="sidebar-toggle"
        glyph={ICONS.sidebar}
        tokens={tokens}
        onClick={() => run('view.toggleVerticalTabs')}
      />
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
        {title}
      </text>
      <IconButton
        testId="new-tab"
        glyph={ICONS.newTab}
        tokens={tokens}
        onClick={() => run('tab.new')}
      />
    </div>
  );
}
