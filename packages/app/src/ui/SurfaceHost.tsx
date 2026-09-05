/**
 * Surface host (05 §4 as amended by Q44 and ADR 0009).
 *
 * Renders the active Tab's Pane tree: a Split is a flex row/column whose
 * children take `ratio` / `1 - ratio` of it around a draggable Divider, and a
 * leaf is one `<terminal-grid>`. Every Pane of the visible Tab is mounted;
 * nothing from other Tabs is. Warm Replicas for those live in Rust
 * (`st-client-core` keeps an LRU), and the other Tabs of the active Session
 * are attached Passive.
 *
 * Only the focused Pane's grid is `focused`, and only while neither the
 * palette nor the Menu owns the keyboard. Clicking anywhere in a Pane focuses
 * it: the wrapper's `onMouseDown` fires alongside the grid's own handling
 * because gpuix never stops propagation (layout skill, trap #7) — here that is
 * what we want.
 *
 * Selection and scroll offset are NOT handled here — they travel on the data
 * plane (Q43); this component never sees cell data at all (Q10, Q13).
 */

import { useRef, useSyncExternalStore } from 'react';
import type { Layout, SplitPath } from '@superterminal/protocol-ts';
import {
  clampRatio,
  contentRect,
  dragRatio,
  pathKey,
  splitGeometry,
  type SplitGeometry,
} from '../state/layout.js';
import { selectActiveTab, selectFocusedSurfaceId } from '../state/selectors.js';
import type { TabId } from '../state/types.js';
import { buildTerminalTheme, type TerminalTheme } from '../theme/tokens.js';
import '../native/terminal-grid.js';
import { Divider, GRAB } from './Divider.js';
import { debug } from '../util/debug.js';

const layoutLog = debug('st:layout');
import { useServices, useWorkspace } from './context.js';

export function SurfaceHost() {
  const { tokens, store } = useServices();
  const tab = useWorkspace(selectActiveTab);
  const focusedId = useWorkspace((s) => (tab ? selectFocusedSurfaceId(s, tab.id) : null));
  const preview = useWorkspace((s) => s.ui.ratioPreview);
  const keyboardTaken = useWorkspace((s) => s.ui.paletteOpen || s.ui.menu !== null);
  // Primitives, not an object: `useSyncExternalStore` compares by identity.
  const windowWidth = useWorkspace((s) => s.ui.window.width);
  const windowHeight = useWorkspace((s) => s.ui.window.height);
  const vertical = useWorkspace((s) => s.ui.verticalTabs);
  const sidebarWidth = useWorkspace((s) => s.ui.sidebarWidth);

  if (!tab || focusedId === null) {
    return (
      <div
        testId="surface-host-empty"
        style={{
          display: 'flex',
          flexGrow: 1,
          alignItems: 'center',
          justifyContent: 'center',
        }}
      >
        <text style={{ color: tokens.fg.muted, fontSize: tokens.font.chrome }}>No open tabs</text>
      </div>
    );
  }

  const tabPreview = preview && preview.tabId === tab.id ? preview : null;
  const rect = contentRect({ width: windowWidth, height: windowHeight }, vertical, sidebarWidth, tokens);
  const geometry = splitGeometry(tab.layout, rect, GRAB, tabPreview);
  if (layoutLog.enabled) layoutLog('rect', JSON.stringify(rect), 'geometry', JSON.stringify([...geometry]));

  return (
    <div testId="surface-host" style={{ flexGrow: 1, minWidth: 0, minHeight: 0, display: 'flex' }}>
      <PaneTree
        tabId={tab.id}
        node={tab.layout}
        path={[]}
        focusedId={focusedId}
        focusable={!keyboardTaken}
        preview={tabPreview}
        geometry={geometry}
        onFocus={(surfaceId) => store.dispatch({ type: 'pane.focus', tabId: tab.id, surfaceId })}
      />
    </div>
  );
}

interface PaneTreeProps {
  tabId: TabId;
  node: Layout;
  path: SplitPath;
  focusedId: number;
  focusable: boolean;
  preview: { path: SplitPath; ratio: number } | null;
  geometry: Map<string, SplitGeometry>;
  onFocus: (surfaceId: number) => void;
}

function PaneTree(props: PaneTreeProps) {
  const { node, path } = props;
  if (node.kind === 'leaf') {
    return (
      <Pane
        surfaceId={node.surface}
        focused={node.surface === props.focusedId}
        focusable={props.focusable}
        onFocus={() => props.onFocus(node.surface)}
      />
    );
  }

  const key = pathKey(path);
  const ratio = clampRatio(
    props.preview && pathKey(props.preview.path) === key ? props.preview.ratio : node.ratio,
  );
  return (
    <div
      testId={`split-${props.tabId}-${key || 'root'}`}
      style={{
        display: 'flex',
        flexDirection: node.axis,
        flexGrow: 1,
        minWidth: 0,
        minHeight: 0,
        overflow: 'hidden',
      }}
    >
      <div style={{ display: 'flex', flexGrow: ratio, flexBasis: 0, minWidth: 0, minHeight: 0, overflow: 'hidden' }}>
        <PaneTree {...props} node={node.first} path={[...path, 0]} />
      </div>
      <SplitDivider
        tabId={props.tabId}
        path={path}
        axis={node.axis}
        ratio={node.ratio}
        geometry={props.geometry.get(key) ?? null}
      />
      <div style={{ display: 'flex', flexGrow: 1 - ratio, flexBasis: 0, minWidth: 0, minHeight: 0, overflow: 'hidden' }}>
        <PaneTree {...props} node={node.second} path={[...path, 1]} />
      </div>
    </div>
  );
}

/**
 * Drag = live `ratio.preview` (local, one per pointer move) then ONE
 * `tab.set_ratio` on release, so the grids re-flow at most twice.
 */
function SplitDivider(props: {
  tabId: TabId;
  path: SplitPath;
  axis: 'row' | 'column';
  ratio: number;
  geometry: SplitGeometry | null;
}) {
  const { tokens, store, commandContext } = useServices();
  // A ref: every `ratio.preview` re-renders this component, and the drag
  // origin must survive that.
  const start = useRef({ pos: 0, ratio: clampRatio(props.ratio) });
  const extent = props.geometry?.extent ?? 0;

  return (
    <Divider
      testId={`divider-${props.tabId}-${pathKey(props.path) || 'root'}`}
      axis={props.axis}
      tokens={tokens}
      onDragStart={(pos) => {
        start.current = { pos, ratio: clampRatio(props.ratio) };
      }}
      onDrag={(pos) => {
        store.dispatch({
          type: 'ratio.preview',
          tabId: props.tabId,
          path: props.path,
          ratio: dragRatio(start.current.ratio, start.current.pos, pos, extent),
        });
      }}
      onDragEnd={() => {
        const preview = store.getState().ui.ratioPreview;
        store.dispatch({ type: 'ratio.clear' });
        if (!preview || preview.tabId !== props.tabId || pathKey(preview.path) !== pathKey(props.path)) {
          return;
        }
        if (preview.ratio === clampRatio(props.ratio)) return;
        void commandContext.client
          .request('tab.set_ratio', { tab: props.tabId, path: props.path, ratio: preview.ratio })
          .catch((err: unknown) => {
            store.dispatch({
              type: 'toast.push',
              text: `Could not resize: ${err instanceof Error ? err.message : String(err)}`,
              kind: 'error',
            });
          });
      }}
    />
  );
}

function Pane(props: {
  surfaceId: number;
  focused: boolean;
  focusable: boolean;
  onFocus: () => void;
}) {
  const { tokens, registry, config, store, commandBus, socketPath } = useServices();
  const command = useSyncExternalStore(
    commandBus.subscribe,
    commandBus.getSnapshot,
    commandBus.getSnapshot,
  );
  const theme: TerminalTheme = buildTerminalTheme(config.theme, config.terminal.boldIsBright);
  const id = props.surfaceId;

  return (
    <div
      testId={`pane-${id}`}
      onMouseDown={props.onFocus}
      style={{
        display: 'flex',
        flexGrow: 1,
        minWidth: 0,
        minHeight: 0,
        overflow: 'hidden',
        // A hairline of accent on the focused Pane's edge, only when there is
        // more than one Pane to tell apart, would be nicer; a border on every
        // Pane shifts the grid by 1px, so the focus cue is the cursor for now.
        backgroundColor: tokens.bg.glass,
      }}
    >
      <terminal-grid
        // Remount on surface change so the element never carries stale state.
        key={id}
        testId={`terminal-grid-${id}`}
        surfaceId={id}
        // The element opens its OWN data-plane connection (Q13/Q14): cell data
        // never passes through JavaScript, so it needs the socket path, not a
        // handle to our control-plane client.
        socketPath={socketPath}
        // Refocus the grid after a tab switch remounts it (`key`), so the
        // focus chain is never empty and root shortcuts keep working without
        // a click. Gated on the palette/Menu so their `<input>` can hold focus.
        focused={props.focused && props.focusable}
        {...(config.font.family ? { fontFamily: config.font.family } : {})}
        fontSize={config.font.size}
        lineHeight={config.font.lineHeight}
        theme={theme}
        cursorStyle={config.terminal.cursorStyle}
        cursorBlink={config.terminal.cursorBlink}
        scrollbar={config.terminal.scrollbar}
        padding={{ top: 4, right: 8, bottom: 4, left: 8 }}
        passthroughKeys={registry.passthroughShortcuts}
        {...(command && command.surfaceId === id
          ? { command: { seq: command.seq, name: command.name, args: command.args } }
          : {})}
        style={{ flexGrow: 1 }}
        onBell={() => store.dispatch({ type: 'surface.bell', surfaceId: id })}
        onFocus={() => {
          store.dispatch({ type: 'surface.clearBell', surfaceId: id });
          props.onFocus();
        }}
      />
    </div>
  );
}
