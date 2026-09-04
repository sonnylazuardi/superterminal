/**
 * Chrome icons.
 *
 * Its own module rather than a helper inside `App.tsx`, because `App` imports
 * `TabStrip` — the reverse import would be a cycle.
 *
 * # Why glyphs are centred explicitly
 *
 * `alignItems: 'center'` centres the text ELEMENT inside the button, not the
 * glyph inside its line box. A glyph whose font's ascent/descent are asymmetric
 * (or whose advance is wider than its ink) still sits high, low or off to one
 * side, which is exactly what the first pass looked like. So every icon gets a
 * deterministic box instead: `width: '100%'` + `textAlign: 'center'` fixes it
 * horizontally, and `lineHeight` equal to the button side fixes it vertically —
 * both are real gpuix style props (`style.rs` `text_align` / `line_height`).
 *
 * # Why these particular characters
 *
 * Chrome text renders in the system UI font, and a character it does not cover
 * is resolved through a fallback face with unrelated metrics — the reason the
 * first pass mixed a fullwidth `＋` (U+FF0B), dingbats (`❯` U+276F, `❏` U+274F)
 * and geometric shapes (`▤` U+25A4) and got four different visual baselines.
 * Everything below is ASCII, Latin-1 or common punctuation that SF Pro covers,
 * so they all shape from one face.
 */

import type { Tokens } from '../theme/tokens.js';

export const ICONS = {
  /** Toggle the sidebar / tab orientation. */
  sidebar: '≡', // ≡ IDENTICAL TO
  /** New tab. ASCII '+', not the fullwidth form. */
  newTab: '+',
  /** Command palette. */
  palette: '⋯', // ⋯ MIDLINE HORIZONTAL ELLIPSIS
  /** New session. */
  newSession: '□', // □ WHITE SQUARE
  /** Leading marker on a sidebar tab row. */
  chevron: '›', // › SINGLE RIGHT-POINTING ANGLE QUOTATION MARK
  /** Close a tab. Latin-1 multiplication sign, not a dingbat cross. */
  close: '×', // ×
  /** The active Surface, in the content header. */
  surface: '□', // □
} as const;

/**
 * Left/right inset for the sidebar's header and footer icon buttons.
 *
 * Derived, not eyeballed, so the buttons' glyph centres land on the SAME
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

export interface GlyphProps {
  glyph: string;
  /** Font size in px. */
  size: number;
  color: string;
  /**
   * Side of the square the glyph is centred in. Becomes `lineHeight`, which is
   * what actually centres it vertically.
   */
  box: number;
}

export function Glyph(props: GlyphProps) {
  return (
    <text
      style={{
        color: props.color,
        fontSize: props.size,
        width: '100%',
        textAlign: 'center',
        lineHeight: props.box,
      }}
    >
      {props.glyph}
    </text>
  );
}

export interface IconButtonProps {
  testId: string;
  glyph: string;
  tokens: Tokens;
  onClick: () => void;
  /** Defaults to `tokens.strip.iconButton`. */
  size?: number;
  /** Defaults to `tokens.font.chip`. */
  fontSize?: number;
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
        // The glyph centres itself (see the module note); these keep the text
        // element itself filling the button so `width: '100%'` means the button.
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        borderRadius: tokens.radius.tab,
        cursor: 'pointer',
        hover: { backgroundColor: tokens.bg.glassHover },
      }}
    >
      <Glyph
        glyph={props.glyph}
        size={props.fontSize ?? tokens.font.chip}
        color={props.color ?? tokens.fg.muted}
        box={size}
      />
    </div>
  );
}
