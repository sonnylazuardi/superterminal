/**
 * The tab-row Menu (CONTEXT.md "Menu"): a floating list of Commands opened at
 * the pointer by a right-click. Fully keyboard-driven — ↑/↓ move, Enter runs,
 * Esc or a click elsewhere closes — using the same trick as the palette: an
 * `<input autoFocus>` owns the keyboard while the Menu is open. It is parked
 * inside a zero-height clipped box so nothing of it paints. Enter arrives as
 * `onSubmit`, never `onKeyDown` (the gpuix input binds it to its Submit
 * action; see CommandPalette).
 *
 * Items run on the RIGHT-CLICKED Tab: it is activated first when it is not
 * the active one, then the command runs with the Tab id as its argument.
 */

import { menuItemsFor, type MenuItem } from '../state/layout.js';
import { useRunCommand, useServices, useWorkspace } from './context.js';

const WIDTH = 220;

export function Menu() {
  const { tokens, registry, store, commandContext } = useServices();
  const menu = useWorkspace((s) => s.ui.menu);
  // Select the Tab (a stable reference), then derive the rows: a selector
  // returning a fresh array would re-render forever.
  const menuTab = useWorkspace((s) => (s.ui.menu ? s.tabs[s.ui.menu.tabId] : undefined));
  const items = menuItemsFor(menuTab);
  const run = useRunCommand();

  if (!menu) return null;
  const index = Math.min(menu.index, Math.max(0, items.length - 1));

  const activate = (item: MenuItem) => {
    const state = store.getState();
    const tab = state.tabs[menu.tabId];
    store.dispatch({ type: 'menu.close' });
    if (!tab) return;
    const isActive =
      state.activeSessionId === tab.sessionId && state.activeTabBySession[tab.sessionId] === tab.id;
    const go = () => run(item.commandId, tab.id);
    if (isActive) {
      go();
      return;
    }
    // Focus follows the right-click: show the Tab the command acts on.
    void commandContext.client
      .request('tab.set_active', { tab: tab.id })
      .then(go)
      .catch((err: unknown) => {
        store.dispatch({
          type: 'toast.push',
          text: err instanceof Error ? err.message : String(err),
          kind: 'error',
        });
      });
  };

  return (
    <anchored
      testId="tab-menu"
      anchor="topLeft"
      position={{ x: menu.x, y: menu.y }}
      style={{
        width: WIDTH,
        display: 'flex', // REQUIRED or children are blocks
        flexDirection: 'column',
        backgroundColor: tokens.bg.overlay,
        borderRadius: tokens.radius.tab,
        borderWidth: tokens.border.width,
        borderColor: tokens.border.glass,
        overflow: 'hidden',
      }}
    >
      {/* `<anchored>` supports only click/enter/leave, so the outside-click
          listener lives on this inner div: GPUI runs `on_mouse_down_out` in
          the capture phase, so a press anywhere else — a terminal grid
          included — closes the Menu even if the target consumes the press. */}
      <div
        testId="tab-menu-body"
        onMouseDownOutside={() => store.dispatch({ type: 'menu.close' })}
        style={{
          display: 'flex',
          flexDirection: 'column',
          padding: tokens.space.sm,
          gap: tokens.space.xs,
        }}
      >
        {items.map((item, i) => {
          const selected = i === index;
          return (
            <div
              key={item.commandId}
              testId={`menu-item-${item.commandId}`}
              onClick={() => activate(item)}
              style={{
                display: 'flex',
                flexDirection: 'row',
                alignItems: 'center',
                height: tokens.strip.rowHeight,
                paddingLeft: tokens.space.lg,
                paddingRight: tokens.space.lg,
                gap: tokens.space.lg,
                borderRadius: tokens.radius.chipSmall,
                backgroundColor: selected ? tokens.bg.glassActive : 'transparent',
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
                {item.title}
              </text>
              <text
                style={{
                  color: tokens.fg.muted,
                  fontSize: tokens.font.chip,
                  flexShrink: 0,
                }}
              >
                {registry.shortcutHint(item.commandId)}
              </text>
            </div>
          );
        })}
        {/* Keyboard owner: parked in a clipped zero-height box so it never paints. */}
        <div style={{ height: 0, overflow: 'hidden' }}>
          <input
            testId="menu-keys"
            autoFocus
            value=""
            style={{ height: 1, width: 1 }}
            onSubmit={() => {
              const item = items[index];
              if (item) activate(item);
            }}
            onKeyDown={(event) => {
              switch (event.key) {
                case 'escape':
                  store.dispatch({ type: 'menu.close' });
                  return;
                case 'down':
                  store.dispatch({
                    type: 'menu.move',
                    delta: 1,
                    count: items.length,
                  });
                  return;
                case 'up':
                  store.dispatch({
                    type: 'menu.move',
                    delta: -1,
                    count: items.length,
                  });
                  return;
                default:
              }
            }}
          />
        </div>
      </div>
    </anchored>
  );
}
