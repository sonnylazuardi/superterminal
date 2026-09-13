import { describe, expect, test } from 'bun:test';
import { isOpenableUrl, openCommandFor, openExternal } from './open-external.js';

const URL_ = 'https://github.com/sonnylazuardi/superterminal';

describe('openExternal', () => {
  test('uses the platform launcher', () => {
    expect(openCommandFor(URL_, 'darwin')).toEqual(['open', URL_]);
    expect(openCommandFor(URL_, 'win32')).toEqual(['rundll32.exe', 'url.dll,FileProtocolHandler', URL_]);
    expect(openCommandFor(URL_, 'linux')).toEqual(['xdg-open', URL_]);
    expect(openCommandFor(URL_, 'linux', true)).toEqual(['wslview', URL_]);
  });

  test('spawns once and reports success', () => {
    const calls: string[][] = [];
    expect(openExternal(URL_, 'linux', { spawn: (argv) => calls.push(argv) })).toBe(true);
    expect(calls).toEqual([['xdg-open', URL_]]);
  });

  test('a launcher that cannot start is reported, not thrown', () => {
    expect(
      openExternal(URL_, 'linux', {
        spawn: () => {
          throw new Error('ENOENT');
        },
      }),
    ).toBe(false);
  });

  test('only http(s) URLs leave the app', () => {
    expect(isOpenableUrl(URL_)).toBe(true);
    expect(isOpenableUrl('http://example.com')).toBe(true);
    expect(isOpenableUrl('file:///etc/passwd')).toBe(false);
    expect(isOpenableUrl('javascript:alert(1)')).toBe(false);
    expect(isOpenableUrl('not a url')).toBe(false);
    const calls: string[][] = [];
    expect(openExternal('file:///etc/passwd', 'linux', { spawn: (argv) => calls.push(argv) })).toBe(false);
    expect(calls).toEqual([]);
  });
});
