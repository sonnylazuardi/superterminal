/**
 * Chrome icons: Lucide (https://lucide.dev, ISC licence) rendered as SVG.
 *
 * Its own module rather than a helper inside `App.tsx`, because `App` imports
 * `TabStrip` — the reverse import would be a cycle.
 *
 * gpuix's `<svg source>` hands the markup to GPUI, which paints it as a mask
 * tinted with the element's `color` — so one icon set serves every state
 * (muted, accent, danger) and it scales crisply at any DPI, unlike the text
 * glyphs the chrome used before (which shaped from whichever font covered
 * the character and never sat on one baseline).
 *
 * The inner markup is copied verbatim from `lucide/icons/<name>.svg`; the
 * wrapper below supplies Lucide's standard attributes (24-unit viewBox, 2px
 * round stroke, no fill).
 */

import type { Tokens } from '../theme/tokens.js';

/** Inner markup of each Lucide icon, keyed by the name it has in Lucide. */
const LUCIDE = {
  'panel-left': '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/>',
  plus: '<path d="M5 12h14"/><path d="M12 5v14"/>',
  ellipsis:
    '<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>',
  'square-plus':
    '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M8 12h8"/><path d="M12 8v8"/>',
  'chevron-right': '<path d="m9 18 6-6-6-6"/>',
  x: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
  user: '<path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>',
  terminal: '<path d="M12 19h8"/><path d="m4 17 6-6-6-6"/>',
  'columns-2': '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M12 3v18"/>',
} as const;

export type LucideName = keyof typeof LUCIDE;

/** What each chrome affordance uses. Change the icon here, not at the call site. */
export const ICONS = {
  /** Toggle the sidebar / tab orientation. */
  sidebar: 'panel-left',
  /** New tab. */
  newTab: 'plus',
  /** Command palette. */
  palette: 'ellipsis',
  /** New session. */
  newSession: 'square-plus',
  /** Leading marker on a sidebar tab row. */
  chevron: 'chevron-right',
  /** Close a tab. */
  close: 'x',
  /** The active Surface, in the content header. */
  surface: 'terminal',
  /** The Session (a "profile"), beside its name. */
  session: 'user',
  /** A Tab with more than one Pane. */
  panes: 'columns-2',
} as const satisfies Record<string, LucideName>;

/** Full SVG document for a Lucide icon; `currentColor` is tinted by GPUI. */
export function lucideSvg(name: LucideName, strokeWidth = 2): string {
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" ` +
    `stroke="currentColor" stroke-width="${strokeWidth}" stroke-linecap="round" ` +
    `stroke-linejoin="round">${LUCIDE[name]}</svg>`
  );
}

/**
 * Left/right inset for the sidebar's header and footer icon buttons.
 *
 * Derived, not eyeballed, so the buttons' icon centres land on the SAME
 * vertical line as the tab rows' leading icons. A row's icon centre sits at
 * `sidebarPadding + rowPaddingX + rowIcon/2`; an icon button's centre sits at
 * `inset + iconButton/2`. Solving for `inset` gives the expression below —
 * without it the footer icons sat ~6pt left of the row icons.
 */
export function sidebarIconInset(tokens: Tokens): number {
  return (
    tokens.strip.sidebarPadding +
    tokens.strip.rowPaddingX +
    (tokens.strip.rowIcon - tokens.strip.iconButton) / 2
  );
}

export interface IconProps {
  name: LucideName;
  /** Side of the square the icon is drawn in, px. */
  size: number;
  color: string;
  /** Lucide's default is 2; thinner reads better at small sizes. */
  strokeWidth?: number;
  testId?: string;
}

export function Icon(props: IconProps) {
  return (
    <svg
      {...(props.testId ? { testId: props.testId } : {})}
      source={lucideSvg(props.name, props.strokeWidth)}
      style={{ width: props.size, height: props.size, flexShrink: 0, color: props.color }}
    />
  );
}

export interface IconButtonProps {
  testId: string;
  icon: LucideName;
  tokens: Tokens;
  onClick: () => void;
  /** Button side; defaults to `tokens.strip.iconButton`. */
  size?: number;
  /** Icon side; defaults to `tokens.icon.button`. */
  iconSize?: number;
  color?: string;
}

export function IconButton(props: IconButtonProps) {
  const { tokens } = props;
  const size = props.size ?? tokens.strip.iconButton;
  return (
    <div
      testId={props.testId}
      onClick={props.onClick}
      style={{
        width: size,
        height: size,
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        borderRadius: tokens.radius.tab,
        cursor: 'pointer',
        hover: { backgroundColor: tokens.bg.glassHover },
      }}
    >
      <Icon
        name={props.icon}
        size={props.iconSize ?? tokens.icon.button}
        color={props.color ?? tokens.fg.muted}
      />
    </div>
  );
}
