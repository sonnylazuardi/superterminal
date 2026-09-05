/**
 * Chrome design tokens (05 §7). Derived from `blurred-window.tsx`.
 *
 * Never inline a hex value in a component: the alpha-white "glass" palette
 * reads as grey-on-black on an opaque window, so `<App>` picks `glassTokens` or
 * `opaqueTokens` from the resolved window background and passes them down.
 * The terminal palette itself is a `theme` prop on `<terminal-grid>` (04) and
 * is independent of these.
 */

export interface Tokens {
  bg: {
    glass: string;
    glassHover: string;
    glassActive: string;
    /** Chips inside an already-glass surface (row icons); softer than `glass`. */
    glassSubtle: string;
    overlay: string;
  };
  border: { glass: string; width: number };
  fg: { primary: string; muted: string; danger: string };
  accent: string;
  radius: { panel: number; tab: number; chip: number; chipSmall: number };
  font: { chrome: number; chip: number; paletteInput: number; family?: string };
  /** Lucide icon sides, px (the drawn square, not the button around it). */
  icon: { button: number; row: number; chip: number; section: number };
  space: { xs: number; sm: number; md: number; lg: number; xl: number };
  padding: { trafficLights: number };
  strip: {
    height: number;
    titleBarHeight: number;
    footerHeight: number;
    iconButton: number;
    sidebarPadding: number;
    sidebarSectionLabel: number;
    sectionHeaderHeight: number;
    rowIcon: number;
    rowPaddingX: number;
    rowHeight: number;
    paletteInputHeight: number;
    chipHeight: number;
    renameWidth: number;
    tabHeight: number;
    toastMaxWidth: number;
    tabMaxWidth: number;
    tabMinWidth: number;
    gap: number;
    paddingX: number;
    verticalWidth: number;
  };
}

const SPACE = { xs: 2, sm: 4, md: 6, lg: 8, xl: 12 } as const;

// Pixel-read of the traffic-light group with trafficLightX: 18.
// Buttons are 14pt across on a 23pt pitch: x 18..32, 41..55, 64..78.
// Group ends at 78, NOT the textbook 70 (3×12 on a 20pt pitch).
const TRAFFIC_LIGHTS_RIGHT = 78;

const shared = {
  fg: { primary: '#F2F2F2', muted: '#FFFFFF80', danger: '#FF6B6B' },
  accent: '#7AA2F7',
  radius: { panel: 16, tab: 8, chip: 999, chipSmall: 6 },
  font: { chrome: 12.5, chip: 11.5, paletteInput: 14 },
  icon: { button: 18, row: 16, chip: 13, section: 14 },
  space: SPACE,
  padding: { trafficLights: TRAFFIC_LIGHTS_RIGHT + SPACE.lg }, // 86
  strip: {
    height: 36,
    titleBarHeight: 38, // buttons sit y 13..27; 38 → symmetric band
    footerHeight: 32,
    iconButton: 28,
    sidebarPadding: 8,
    sidebarSectionLabel: 11,
    sectionHeaderHeight: 24,
    rowIcon: 22,
    rowPaddingX: 8,
    rowHeight: 30,
    paletteInputHeight: 32,
    chipHeight: 22,
    renameWidth: 140,
    tabHeight: 28,
    toastMaxWidth: 320,
    tabMaxWidth: 220,
    tabMinWidth: 90,
    gap: 4,
    paddingX: 12,
    verticalWidth: 220,
  },
} as const;

export const glassTokens: Tokens = {
  bg: {
    glass: '#FFFFFF0D',
    glassHover: '#FFFFFF14',
    glassActive: '#FFFFFF26',
    glassSubtle: '#FFFFFF14',
    overlay: '#16161EFA',
  },
  border: { glass: '#FFFFFF1F', width: 1 },
  ...shared,
};

/** Linux non-blurred / `'opaque'`: the glass alphas need real colours. */
export const opaqueTokens: Tokens = {
  bg: {
    glass: '#1E1E22',
    glassHover: '#26262C',
    glassActive: '#2A2A30',
    glassSubtle: '#24242A',
    overlay: '#16161E',
  },
  border: { glass: '#2E2E36', width: 1 },
  ...shared,
};

export type WindowBackgroundMode = 'blurred' | 'transparent' | 'opaque';

export function tokensFor(background: WindowBackgroundMode): Tokens {
  return background === 'opaque' ? opaqueTokens : glassTokens;
}

/** Terminal palette defaults (04 §10); overridden by `config.theme`. */
export const DEFAULT_TERMINAL_THEME = {
  bg: '#1e1e1e',
  fg: '#d4d4d4',
  cursor: '#d4d4d4',
  cursorText: '#1e1e1e',
  selectionBg: '#3a3d41',
  ansi: [
    '#000000',
    '#cd3131',
    '#0dbc79',
    '#e5e510',
    '#2472c8',
    '#bc3fbc',
    '#11a8cd',
    '#e5e5e5',
    '#666666',
    '#f14c4c',
    '#23d18b',
    '#f5f543',
    '#3b8eea',
    '#d670d6',
    '#29b8db',
    '#e5e5e5',
  ],
} as const;

export interface TerminalTheme {
  ansi: string[];
  fg: string;
  bg: string;
  cursor: string;
  cursorText: string;
  selectionBg: string;
  selectionFg?: string;
  boldIsBright: boolean;
}

/**
 * Build the `<terminal-grid theme=…>` payload from `config.theme`, which is a
 * flat `Record<string,string>` of `ansi0`…`ansi15`, `fg`, `bg`, `cursor`,
 * `cursor_text`/`cursorText`, `selection_bg`/`selectionBg`, `selection_fg`.
 */
export function buildTerminalTheme(
  overrides: Record<string, string> = {},
  boldIsBright = false,
): TerminalTheme {
  const pick = (...keys: string[]): string | undefined => {
    for (const key of keys) {
      const value = overrides[key];
      if (value) return value;
    }
    return undefined;
  };
  const ansi = DEFAULT_TERMINAL_THEME.ansi.map((fallback, i) => overrides[`ansi${i}`] ?? fallback);
  const selectionFg = pick('selection_fg', 'selectionFg');
  return {
    ansi,
    fg: pick('fg', 'foreground') ?? DEFAULT_TERMINAL_THEME.fg,
    bg: pick('bg', 'background') ?? DEFAULT_TERMINAL_THEME.bg,
    cursor: pick('cursor') ?? DEFAULT_TERMINAL_THEME.cursor,
    cursorText: pick('cursor_text', 'cursorText') ?? DEFAULT_TERMINAL_THEME.cursorText,
    selectionBg: pick('selection_bg', 'selectionBg') ?? DEFAULT_TERMINAL_THEME.selectionBg,
    ...(selectionFg ? { selectionFg } : {}),
    boldIsBright,
  };
}
