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
  bg: { glass: string; glassHover: string; glassActive: string; overlay: string };
  border: { glass: string; width: number };
  fg: { primary: string; muted: string; danger: string };
  accent: string;
  radius: { panel: number; tab: number; chip: number };
  font: { chrome: number; chip: number; paletteInput: number; family?: string };
  /** macOS traffic lights need 58 px of headroom. */
  padding: { trafficLights: number };
  strip: {
    height: number;
    tabHeight: number;
    tabMaxWidth: number;
    gap: number;
    paddingX: number;
    verticalWidth: number;
  };
}

const shared = {
  fg: { primary: '#F2F2F2', muted: '#FFFFFF80', danger: '#FF6B6B' },
  accent: '#7AA2F7',
  radius: { panel: 16, tab: 8, chip: 999 },
  font: { chrome: 12.5, chip: 11.5, paletteInput: 14 },
  padding: { trafficLights: 58 },
  strip: {
    height: 36,
    tabHeight: 28,
    tabMaxWidth: 220,
    gap: 4,
    paddingX: 12,
    verticalWidth: 220,
  },
} as const;

export const glassTokens: Tokens = {
  bg: {
    glass: '#FFFFFF0D',
    glassHover: '#FFFFFF1A',
    glassActive: '#FFFFFF26',
    overlay: '#16161ECC',
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
