/**
 * A draggable divider: the sidebar's right edge and every Pane Split use it.
 *
 * Visually it is the usual 1px `border.glass` line; the hit area is a wider
 * invisible band (`GRAB` px) so the pointer does not have to land on the
 * hairline. The band takes the press AND the moves/release: `onMouseDown` +
 * `onMouseMove` give it GPUI pointer capture (see `drag.ts` for why an
 * ancestor cannot do this).
 *
 *   mouseDown → onDragStart(pos)   pos is the window coordinate along the axis
 *   mouseMove → onDrag(pos)        only while the primary button is held
 *   mouseUp   → onDragEnd()
 *
 * Sizing: the band is stretched along the axis by the parent flex container
 * (`alignSelf: 'stretch'`), never `height/width: '100%'` — the parent is
 * sized by `flexGrow`, so its extent is indefinite during layout and taffy
 * resolves the percentage to `auto` → a 7×0 band with an empty hitbox.
 *
 * The component keeps no geometry of its own: what a position *means* is the
 * caller's business (a sidebar width, a Split ratio).
 */

import { DIVIDER_GRAB as GRAB, dragPosition } from '../state/layout.js';
import type { Tokens } from '../theme/tokens.js';
import { debug } from '../util/debug.js';
import { dragController } from './drag.js';

const dragLog = debug('st:drag');

export { GRAB };

/** Mouse payload fields the divider reads; gpuix's `EventPayload` is a superset. */
export interface DividerMouseEvent {
  x?: number;
  y?: number;
  button?: number;
  clickCount?: number;
}

export interface DividerProps {
  testId: string;
  /** `row`: a vertical line dragged along x. `column`: a horizontal line dragged along y. */
  axis: 'row' | 'column';
  tokens: Tokens;
  onDragStart?: (pos: number) => void;
  onDrag: (pos: number) => void;
  onDragEnd?: () => void;
  onDoubleClick?: () => void;
}

const PRIMARY = 0;

export function Divider(props: DividerProps) {
  const { tokens, axis } = props;
  const horizontalLine = axis === 'column';

  const onMouseDown = (event: DividerMouseEvent) => {
    dragLog('press', props.testId, 'button', event.button, 'clicks', event.clickCount, 'at', event.x, event.y);
    if (event.button !== undefined && event.button !== PRIMARY) return;
    if ((event.clickCount ?? 1) >= 2) {
      props.onDoubleClick?.();
      return;
    }
    const pos = dragPosition(axis, event);
    dragLog('down', props.testId, 'at', pos);
    if (pos === null) return;
    props.onDragStart?.(pos);
    // Moves and the release arrive through the app root (see drag.ts).
    dragController.begin(axis, {
      onMove: (p) => props.onDrag(p),
      onEnd: () => {
        dragLog('end', props.testId);
        props.onDragEnd?.();
      },
    });
  };

  return (
    <div
      testId={props.testId}
      // onMouseDown + onMouseMove make gpuix capture the pointer (GPUI
      // `capture_pointer`), so the moves and the release keep coming to this
      // band after the pointer leaves it — ancestors are never "hovered"
      // here because filled elements occlude the hit test.
      onMouseDown={onMouseDown}
      onMouseMove={(event: DividerMouseEvent & { pressedButton?: number }) =>
        dragController.move(event)
      }
      onMouseUp={() => dragController.end()}
      ref={(instance: { id?: number } | null) => {
        if (instance && dragLog.enabled) dragLog('mounted', props.testId, 'element', instance.id);
      }}
      style={{
        // The band: `GRAB` thick across the axis and STRETCHED along it by
        // the parent flex container. Not `height/width: '100%'`: the parent
        // is sized by `flexGrow`, so its extent is indefinite during layout,
        // taffy resolves the percentage to `auto`, and the band laid out as
        // 7×0 — a visible line (the glass showing through the gap) but an
        // empty hitbox that GPUI never hovered or pressed.
        display: 'flex',
        flexDirection: horizontalLine ? 'column' : 'row',
        flexShrink: 0,
        alignSelf: 'stretch',
        justifyContent: 'center',
        ...(horizontalLine ? { height: GRAB } : { width: GRAB }),
        cursor: horizontalLine ? 'row-resize' : 'col-resize',
        // Opaque windows (Windows, X11) paint white wherever no element
        // does, so the band needs the surrounding surface's fill rather
        // than being transparent — otherwise it shows as a bright bar.
        backgroundColor: tokens.bg.glass,
      }}
    >
      <div
        style={{
          backgroundColor: tokens.border.glass,
          alignSelf: 'stretch',
          ...(horizontalLine ? { height: tokens.border.width } : { width: tokens.border.width }),
        }}
      />
    </div>
  );
}
