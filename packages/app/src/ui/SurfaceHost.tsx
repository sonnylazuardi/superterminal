/**
 * Surface host (05 §4 as amended by Q44).
 *
 * Exactly ONE `<terminal-grid>` is mounted: the visible tab's. Warm Replicas
 * live in Rust (`st-client-core` keeps an LRU of 4) and other tabs in the
 * active Session are attached Passive, so re-activation applies a Snapshot into
 * an existing allocation instead of rebuilding one. 05's earlier "keep N=4
 * mounted with display:none" is superseded.
 *
 * Selection and scroll offset are NOT handled here — they travel on the data
 * plane (Q43); this component never sees cell data at all (Q10, Q13).
 */

import { useSyncExternalStore } from 'react';
import { buildTerminalTheme } from '../theme/tokens.js';
import { selectActiveSurface } from '../state/selectors.js';
import '../native/terminal-grid.js';
import { useServices, useWorkspace } from './context.js';

export function SurfaceHost() {
  const { tokens, registry, config, store, commandBus } = useServices();
  const surface = useWorkspace(selectActiveSurface);
  const command = useSyncExternalStore(
    commandBus.subscribe,
    commandBus.getSnapshot,
    commandBus.getSnapshot,
  );

  if (!surface) {
    return (
      <div
        testId="surface-host-empty"
        style={{ flexGrow: 1, alignItems: 'center', justifyContent: 'center' }}
      >
        <text style={{ color: tokens.fg.muted, fontSize: tokens.font.chrome }}>No open tabs</text>
      </div>
    );
  }

  const theme = buildTerminalTheme(config.theme, config.terminal.boldIsBright);

  return (
    <div testId="surface-host" style={{ flexGrow: 1, display: 'flex' }}>
      <terminal-grid
        // Remount on surface change so the element never carries stale state.
        key={surface.id}
        testId={`terminal-grid-${surface.id}`}
        surfaceId={surface.id}
        {...(config.font.family ? { fontFamily: config.font.family } : {})}
        fontSize={config.font.size}
        lineHeight={config.font.lineHeight}
        theme={theme}
        cursorStyle={config.terminal.cursorStyle}
        cursorBlink={config.terminal.cursorBlink}
        scrollbar={config.terminal.scrollbar}
        padding={{ top: 4, right: 8, bottom: 4, left: 8 }}
        passthroughKeys={registry.passthroughShortcuts}
        {...(command && command.surfaceId === surface.id
          ? { command: { seq: command.seq, name: command.name, args: command.args } }
          : {})}
        style={{ flexGrow: 1 }}
        onBell={() => store.dispatch({ type: 'surface.bell', surfaceId: surface.id })}
        onFocus={() => store.dispatch({ type: 'surface.clearBell', surfaceId: surface.id })}
      />
    </div>
  );
}
