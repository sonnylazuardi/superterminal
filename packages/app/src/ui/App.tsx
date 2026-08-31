/**
 * Root component (05 §4).
 *
 *   <App>
 *    ├─ <TitleBarSpacer/>        macOS: 58 px for the traffic lights
 *    ├─ <Frame horizontal|vertical>
 *    │   ├─ <TabStrip/>
 *    │   └─ <SurfaceHost/>       exactly one <terminal-grid> (Q44)
 *    ├─ <CommandPalette/>
 *    ├─ <Banner/>
 *    └─ <StatusToasts/>
 *
 * The root `onKeyDown` receives the keystrokes `<terminal-grid>` declined
 * (`passthroughKeys`, 05 §5) and runs the first enabled matching command.
 */

import type { KeyEventLike } from '../platform/keys.js';
import { Banner, StatusToasts } from './Banner.js';
import { CommandPalette } from './CommandPalette.js';
import { SurfaceHost } from './SurfaceHost.js';
import { TabStrip } from './TabStrip.js';
import { AppProvider, useServices, useWorkspace, type AppServices } from './context.js';

export function App(props: { services: AppServices }) {
  return (
    <AppProvider services={props.services}>
      <AppFrame />
    </AppProvider>
  );
}

function AppFrame() {
  const { registry, commandContext, store } = useServices();
  const vertical = useWorkspace((s) => s.ui.verticalTabs);
  const paletteOpen = useWorkspace((s) => s.ui.paletteOpen);

  const onKeyDown = (event: KeyEventLike) => {
    // While the palette is open it owns Esc/↑/↓/Enter; the `<input>` handles
    // them, so only bail out for those keys (⌘K still toggles session mode).
    if (paletteOpen && ['escape', 'up', 'down', 'enter'].includes(event.key ?? '')) return;

    const match = registry.matchKeybinding(event, store.getState());
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
      <TitleBarSpacer />
      <Banner />
      <div
        testId="frame"
        style={{
          display: 'flex',
          flexDirection: vertical ? 'row' : 'column',
          flexGrow: 1,
        }}
      >
        <TabStrip />
        <SurfaceHost />
      </div>
      <CommandPalette />
      <StatusToasts />
    </div>
  );
}

export function TitleBarSpacer() {
  const { tokens, platform } = useServices();
  if (!platform.isMac) return null;
  return <div testId="titlebar-spacer" style={{ height: tokens.padding.trafficLights }} />;
}
