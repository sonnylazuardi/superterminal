/**
 * TypeScript surface of the native `<terminal-grid>` custom element (04 §3).
 *
 * The element owns all cell data and keyboard input; React only sets props and
 * listens for events (Q13, Q23). Selection and scroll offset are NOT sent from
 * here — they travel on the data plane (Q43).
 */

import type { TerminalTheme } from '../theme/tokens.js';

export interface GridPadding {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

/** Payload shape gpuix delivers to `on*` handlers on a custom element. */
export interface GridEvent {
  elementId: number;
  eventType: string;
  [key: string]: unknown;
}

export interface TerminalGridProps {
  key?: string | number | null;
  /** Changing this detaches the old surface and attaches the new one. */
  surfaceId: number;
  /**
   * Unix socket the element dials for its OWN data-plane connection (Q13/Q14).
   * Cell data never crosses into JavaScript, so the element connects itself
   * rather than borrowing the app's control-plane client.
   */
  socketPath?: string;
  fontFamily?: string;
  fontSize?: number;
  lineHeight?: number;
  theme?: TerminalTheme;
  cursorStyle?: 'block' | 'beam' | 'underline';
  cursorBlink?: boolean;
  padding?: GridPadding;
  /** Keystrokes the element must decline so the app can run them (05 §5). */
  passthroughKeys?: string[];
  scrollbar?: 'auto' | 'always' | 'never';
  focused?: boolean;
  /**
   * One-shot imperative commands are modelled as a monotonically increasing
   * prop, the standard trick for commands in a retained tree (04 §3).
   */
  command?: { seq: number; name: 'copy' | 'paste' | 'scrollToBottom' | 'clearScrollback'; args?: unknown };
  style?: Record<string, unknown>;
  testId?: string;
  onFocus?: (event: GridEvent) => void;
  onBlur?: (event: GridEvent) => void;
  onTitle?: (event: GridEvent) => void;
  onExited?: (event: GridEvent) => void;
  onBell?: (event: GridEvent) => void;
  onSelection?: (event: GridEvent) => void;
  onScroll?: (event: GridEvent) => void;
  onResize?: (event: GridEvent) => void;
  onModes?: (event: GridEvent) => void;
  onKeyDown?: (event: GridEvent) => void;
}

// NOTE: only the production jsx-runtime is augmented. `tsc` is configured with
// `jsx: react-jsx`, and TypeScript refuses an augmentation of the dev runtime
// it never loads; the two runtimes share this JSX namespace at runtime anyway.
declare module '@gpuix/react/jsx-runtime' {
  namespace JSX {
    interface IntrinsicElements {
      'terminal-grid': TerminalGridProps;
    }
  }
}
