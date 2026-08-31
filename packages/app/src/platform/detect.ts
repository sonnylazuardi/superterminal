/**
 * Platform probing (05 §1). Everything takes an explicit environment so the
 * tests can pretend to be macOS or WSLg without touching the process.
 */

import { readFileSync } from 'node:fs';

export type Platform = 'darwin' | 'linux' | 'win32';

export interface PlatformInfo {
  platform: Platform;
  isMac: boolean;
  isLinux: boolean;
  /** Running under WSLg — the RDP compositor mishandles window alpha. */
  isWsl: boolean;
  isWayland: boolean;
  /** `bun build --compile` binary rather than `bun app.tsx`. */
  isCompiled: boolean;
}

export interface DetectInput {
  platform?: NodeJS.Platform;
  env?: Record<string, string | undefined>;
  execPath?: string;
  /** Contents of `/proc/version`; injected in tests. */
  procVersion?: string | null;
}

function readProcVersion(): string | null {
  try {
    return readFileSync('/proc/version', 'utf8');
  } catch {
    return null;
  }
}

export function detectPlatform(input: DetectInput = {}): PlatformInfo {
  const raw = input.platform ?? process.platform;
  const platform: Platform = raw === 'darwin' ? 'darwin' : raw === 'win32' ? 'win32' : 'linux';
  const env = input.env ?? (process.env as Record<string, string | undefined>);
  const execPath = input.execPath ?? process.execPath;
  const exe = execPath.split('/').pop() ?? '';
  const procVersion =
    input.procVersion !== undefined
      ? input.procVersion
      : platform === 'linux'
        ? readProcVersion()
        : null;

  return {
    platform,
    isMac: platform === 'darwin',
    isLinux: platform === 'linux',
    isWsl:
      platform === 'linux' &&
      (Boolean(env['WSL_DISTRO_NAME']) ||
        (procVersion ?? '').toLowerCase().includes('microsoft')),
    isWayland: Boolean(env['WAYLAND_DISPLAY']),
    isCompiled: exe !== 'bun' && exe !== 'bun-debug' && exe !== 'node',
  };
}
