/**
 * Hand a URL to the user's default browser (the About dialog's project link).
 *
 * Per platform, the OS's own "open this" verb, so no browser is assumed:
 *   macOS    `open <url>`
 *   Windows  `rundll32 url.dll,FileProtocolHandler <url>` — a GUI-subsystem
 *            exe, so no console window flashes behind the app (the packaged
 *            client is /SUBSYSTEM:WINDOWS); `cmd /c start` would flash one.
 *   Linux    `xdg-open <url>`; under WSLg `wslview` (from the `wslu` package,
 *            preinstalled on Ubuntu WSL images), which opens the URL in the
 *            Windows browser instead of looking for a Linux one.
 *
 * The child is detached and its output dropped: a failure to spawn (no such
 * program) is reported synchronously as `false` so the caller can show the
 * URL instead.
 */

import type { Platform } from './detect.js';

export type Spawner = (argv: string[]) => void;

export function openCommandFor(url: string, platform: Platform, isWsl = false): string[] {
  switch (platform) {
    case 'darwin':
      return ['open', url];
    case 'win32':
      return ['rundll32.exe', 'url.dll,FileProtocolHandler', url];
    case 'linux':
    default:
      return [isWsl ? 'wslview' : 'xdg-open', url];
  }
}

const defaultSpawner: Spawner = (argv) => {
  const child = Bun.spawn(argv, { stdin: 'ignore', stdout: 'ignore', stderr: 'ignore' });
  child.unref();
};

/** Only http(s) leaves the app; anything else is refused rather than handed to the shell. */
export function isOpenableUrl(url: string): boolean {
  try {
    const parsed = new URL(url);
    return parsed.protocol === 'https:' || parsed.protocol === 'http:';
  } catch {
    return false;
  }
}

export function openExternal(
  url: string,
  platform: Platform,
  options: { isWsl?: boolean; spawn?: Spawner } = {},
): boolean {
  if (!isOpenableUrl(url)) return false;
  const spawn = options.spawn ?? defaultSpawner;
  try {
    spawn(openCommandFor(url, platform, options.isWsl ?? false));
    return true;
  } catch {
    return false;
  }
}
