/**
 * `render()` window options (05 §1, Q28).
 *
 * Pure: it takes the detected platform and the config and returns a plain
 * object, so the table below is unit-testable without a window.
 */

import type { Config, WindowBackground } from '../config/schema.js';
import type { WindowBackgroundMode } from '../theme/tokens.js';
import type { PlatformInfo } from './detect.js';

export interface WindowOptions {
  title: string;
  appName: string;
  width?: number;
  height?: number;
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
  if (platform.isMac) {
    const configured = config.window.background;
    return configured === 'auto' ? 'blurred' : configured;
  }
  return resolveLinuxBackground(config.window.background, platform);
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

export function buildWindowOptions(
  config: Config,
  platform: PlatformInfo,
  remembered: { width: number; height: number } | null = null,
): WindowOptions {
  const windowBackground = resolveBackground(config, platform);
  const size = resolveInitialSize(config, remembered);
  return {
    title: APP_TITLE,
    appName: APP_TITLE,
    ...(size ?? {}),
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
