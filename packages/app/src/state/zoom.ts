/**
 * Font zoom (⌘+ / ⌘−, as iTerm2's "Make Text Bigger/Smaller"): the pure half.
 *
 * Zoom is a **delta in points** on top of `config.font.size`, not a multiplier
 * — one press is one point, which is what iTerm and Terminal.app do and what
 * keeps the cell size an integer-ish number of pixels between steps. It lives
 * in Client State (ADR 0008), so it survives a relaunch like the sidebar width,
 * and a later edit of `[font] size` in config still applies underneath it.
 */

/** Most points the zoom may take away from the configured size. */
export const FONT_ZOOM_MIN = -10;
/** Most points the zoom may add. */
export const FONT_ZOOM_MAX = 40;
/** The smallest cell font the grid will be asked for. */
export const FONT_SIZE_MIN = 6;
/** The largest. */
export const FONT_SIZE_MAX = 96;
/** Points per ⌘+ / ⌘− press. */
export const FONT_ZOOM_STEP = 1;

/** A zoom delta pulled into `[FONT_ZOOM_MIN, FONT_ZOOM_MAX]`; garbage is 0. */
export function clampFontZoom(zoom: number): number {
  if (!Number.isFinite(zoom)) return 0;
  return Math.min(FONT_ZOOM_MAX, Math.max(FONT_ZOOM_MIN, Math.round(zoom)));
}

/** The font size the grid gets: the configured size plus the zoom, bounded. */
export function zoomedFontSize(base: number, zoom: number): number {
  const size = (Number.isFinite(base) && base > 0 ? base : 13) + clampFontZoom(zoom);
  return Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, size));
}
