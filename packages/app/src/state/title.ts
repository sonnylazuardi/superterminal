/**
 * What the chrome shows for a Surface's title.
 *
 * Shells set the OSC title to `user@host:path` (zsh/bash defaults); inside a
 * terminal app the host name is noise, so only the path part is shown. A
 * user_title (renamed Surface) or any title that does not have that shape is
 * shown as-is. Pure, so the tab row, content header and title bar agree.
 */

const USER_AT_HOST = /^[^@\s:]+@[^\s:]+:(.*)$/;

export function displayTitle(title: string | undefined, fallback: string): string {
  if (!title) return fallback;
  const match = USER_AT_HOST.exec(title);
  if (match) {
    const path = match[1]?.trim() ?? '';
    return path.length > 0 ? path : fallback;
  }
  return title;
}
