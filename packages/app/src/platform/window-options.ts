/**
 * `render()` window options (05 §1, Q28).
 *
 * Pure: it takes the detected platform and the config and returns a plain
 * object, so the table below is unit-testable without a window.
 */

import type { Config, WindowBackground } from '../config/schema.js';
import type { WindowDisplay, WindowPlacement } from '../state/client-state.js';
import type { WindowBackgroundMode } from '../theme/tokens.js';
import type { PlatformInfo } from './detect.js';

export interface WindowOptions {
  title: string;
  appName: string;
  width?: number;
  height?: number;
  /** Window origin in logical px; only used together with `y`. */
  x?: number;
  y?: number;
  /** The display the window was on, so the native side can re-identify it. */
  display?: WindowDisplay;
  /** Open maximized; the size above stays the windowed restore size. */
  maximized?: boolean;
  minWidth: number;
  minHeight: number;
  windowBackground: WindowBackgroundMode;
  titlebarTransparent: boolean;
  trafficLightX?: number;
  trafficLightY?: number;
  focus: boolean;
}

export const APP_TITLE = 'superterminal';
export const MIN_WIDTH = 480;
export const MIN_HEIGHT = 320;

/**
 * `config.window.background` when it is not `'auto'`; else `'opaque'` under
 * WSLg (the RDP compositor mishandles alpha), else `'transparent'` on Wayland,
 * else `'opaque'` (X11 compositor presence is not cheaply probeable).
 * `'blurred'` is treated as `'transparent'` on Linux.
 */
export function resolveLinuxBackground(
  configured: WindowBackground,
  platform: Pick<PlatformInfo, 'isWsl' | 'isWayland'>,
): WindowBackgroundMode {
  if (configured === 'blurred') return 'transparent';
  if (configured !== 'auto') return configured;
  if (platform.isWsl) return 'opaque';
  if (platform.isWayland) return 'transparent';
  return 'opaque';
}

export function resolveBackground(
  config: Config,
  platform: PlatformInfo,
): WindowBackgroundMode {
  const configured = config.window.background;
  if (platform.platform === 'win32') {
    // A transparent GPUI quad punches through to the desktop on Windows, and
    // the Win32 blur path is unreliable; `auto` and `blurred` are opaque, and
    // only an explicit `transparent` stays transparent.
    return configured === 'transparent' ? 'transparent' : 'opaque';
  }
  if (platform.isMac) {
    return configured === 'auto' ? 'blurred' : configured;
  }
  return resolveLinuxBackground(configured, platform);
}

/**
 * The size to open at. Client State (the last size, ADR 0008) wins over
 * Config, which wins over gpuix's 800×600 default. Whatever the source, it
 * is clamped to the minimum: a remembered size below it would open a window
 * the user could not have made.
 */
export function resolveInitialSize(
  config: Config,
  remembered: { width: number; height: number } | null,
): { width: number; height: number } | null {
  const size =
    remembered ??
    (config.window.width && config.window.height
      ? { width: config.window.width, height: config.window.height }
      : null);
  if (!size) return null;
  return {
    width: Math.max(MIN_WIDTH, Math.round(size.width)),
    height: Math.max(MIN_HEIGHT, Math.round(size.height)),
  };
}

/**
 * The remembered origin, when it can still be trusted.
 *
 * The JS side cannot enumerate the connected displays, so it only rejects an
 * origin whose own remembered `display.bounds` no longer contains the window
 * centre: that is the display-unplugged/rearranged case. When the platform
 * recorded a display `uuid`, or no display bounds at all, the origin is passed
 * through and the native side re-validates against the live display list (and
 * opens centred when nothing matches).
 */
export function resolveWindowOrigin(
  remembered: WindowPlacement | null,
): { x: number; y: number } | null {
  if (!remembered || remembered.x === undefined || remembered.y === undefined) return null;
  const display = remembered.display;
  const bounds = display?.bounds;
  if (bounds && display?.uuid === undefined) {
    const centreX = remembered.x + remembered.width / 2;
    const centreY = remembered.y + remembered.height / 2;
    const inside =
      centreX >= bounds.x &&
      centreX < bounds.x + bounds.width &&
      centreY >= bounds.y &&
      centreY < bounds.y + bounds.height;
    if (!inside) return null;
  }
  return { x: remembered.x, y: remembered.y };
}

export function buildWindowOptions(
  config: Config,
  platform: PlatformInfo,
  remembered: WindowPlacement | null = null,
): WindowOptions {
  const windowBackground = resolveBackground(config, platform);
  const size = resolveInitialSize(config, remembered);
  const origin = resolveWindowOrigin(remembered);
  const display = remembered?.display;
  return {
    title: APP_TITLE,
    appName: APP_TITLE,
    ...(size ?? {}),
    ...(origin ?? {}),
    ...(display ? { display } : {}),
    ...(remembered?.maximized ? { maximized: true } : {}),
    minWidth: MIN_WIDTH,
    minHeight: MIN_HEIGHT,
    windowBackground,
    // macOS draws chrome under the traffic lights; Linux has none, so the tab
    // strip starts at the top.
    titlebarTransparent: platform.isMac,
    ...(platform.isMac ? { trafficLightX: 18, trafficLightY: 13 } : {}),
    focus: true,
  };
}

/** Top padding reserved for the macOS traffic lights (05 §7). */
export function titleBarPadding(platform: PlatformInfo, trafficLights: number): number {
  return platform.isMac ? trafficLights : 0;
}
