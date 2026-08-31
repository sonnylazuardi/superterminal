import { describe, expect, test } from 'bun:test';
import { detectPlatform } from './detect.js';
import {
  bindingMatchesEvent,
  formatKeybinding,
  keybindingEquals,
  normalizeKey,
  parseKeybinding,
  resolveBinding,
  toKeystrokeString,
} from './keys.js';

describe('parseKeybinding', () => {
  test('parses plus- and dash-separated specs', () => {
    expect(parseKeybinding('mod+shift+t')).toEqual({ mods: ['mod', 'shift'], key: 't' });
    expect(parseKeybinding('ctrl-tab')).toEqual({ mods: ['ctrl'], key: 'tab' });
    expect(parseKeybinding('alt+1')).toEqual({ mods: ['alt'], key: '1' });
    expect(parseKeybinding('t')).toEqual({ mods: [], key: 't' });
  });

  test('accepts aliases and normalises case', () => {
    expect(parseKeybinding('Cmd+Shift+P')).toEqual({ mods: ['mod', 'shift'], key: 'p' });
    expect(parseKeybinding('Control+Option+Esc')).toEqual({
      mods: ['ctrl', 'alt'],
      key: 'escape',
    });
    expect(parseKeybinding('mod+Return')).toEqual({ mods: ['mod'], key: 'enter' });
  });

  test('deduplicates repeated modifiers', () => {
    expect(parseKeybinding('mod+mod+shift+shift+x').mods).toEqual(['mod', 'shift']);
  });

  test('bracket keys survive', () => {
    expect(parseKeybinding('mod+shift+]')).toEqual({ mods: ['mod', 'shift'], key: ']' });
    expect(parseKeybinding('mod+shift+[')).toEqual({ mods: ['mod', 'shift'], key: '[' });
  });

  test('rejects unknown modifiers and empty specs', () => {
    expect(() => parseKeybinding('hyper+x')).toThrow(/unknown modifier/);
    expect(() => parseKeybinding('')).toThrow();
  });

  test('normalizeKey lowercases single characters only', () => {
    expect(normalizeKey('T')).toBe('t');
    expect(normalizeKey('F1')).toBe('f1');
    expect(normalizeKey('PgUp')).toBe('pageup');
  });
});

describe('resolveBinding — mod per platform (Q29)', () => {
  const modT = parseKeybinding('mod+t');

  test('mod is Cmd on darwin', () => {
    expect(resolveBinding(modT, 'darwin')).toEqual({
      cmd: true,
      ctrl: false,
      alt: false,
      shift: false,
      key: 't',
    });
  });

  test('mod is Ctrl+Shift on linux', () => {
    expect(resolveBinding(modT, 'linux')).toEqual({
      cmd: false,
      ctrl: true,
      alt: false,
      shift: true,
      key: 't',
    });
  });

  test('mod+shift collapses on linux and stays distinct on darwin', () => {
    const b = parseKeybinding('mod+shift+]');
    expect(resolveBinding(b, 'darwin')).toMatchObject({ cmd: true, shift: true, ctrl: false });
    expect(resolveBinding(b, 'linux')).toMatchObject({ cmd: false, ctrl: true, shift: true });
  });

  test('explicit ctrl bindings are identical on both platforms', () => {
    const b = parseKeybinding('ctrl+tab');
    expect(resolveBinding(b, 'darwin')).toEqual(resolveBinding(b, 'linux'));
  });
});

describe('toKeystrokeString', () => {
  test('emits gpuix keystroke names', () => {
    expect(toKeystrokeString(parseKeybinding('mod+t'), 'darwin')).toBe('cmd-t');
    expect(toKeystrokeString(parseKeybinding('mod+t'), 'linux')).toBe('ctrl-shift-t');
    expect(toKeystrokeString(parseKeybinding('ctrl+tab'), 'linux')).toBe('ctrl-tab');
    expect(toKeystrokeString(parseKeybinding('mod+shift+p'), 'darwin')).toBe('shift-cmd-p');
    expect(toKeystrokeString(parseKeybinding('alt+1'), 'linux')).toBe('alt-1');
  });
});

describe('formatKeybinding', () => {
  test('macOS uses glyphs, Linux words', () => {
    expect(formatKeybinding(parseKeybinding('mod+t'), 'darwin')).toBe('⌘T');
    expect(formatKeybinding(parseKeybinding('mod+shift+p'), 'darwin')).toBe('⇧⌘P');
    expect(formatKeybinding(parseKeybinding('mod+t'), 'linux')).toBe('Ctrl+Shift+T');
    expect(formatKeybinding(parseKeybinding('ctrl+tab'), 'linux')).toBe('Ctrl+⇥');
  });
});

describe('bindingMatchesEvent', () => {
  const ev = (key: string, mods: Partial<Record<'cmd' | 'ctrl' | 'alt' | 'shift', boolean>> = {}) => ({
    key,
    modifiers: { cmd: false, ctrl: false, alt: false, shift: false, ...mods },
  });

  test('matches the resolved combination on each platform', () => {
    const b = parseKeybinding('mod+t');
    expect(bindingMatchesEvent(b, ev('t', { cmd: true }), 'darwin')).toBe(true);
    expect(bindingMatchesEvent(b, ev('t', { ctrl: true, shift: true }), 'linux')).toBe(true);
    expect(bindingMatchesEvent(b, ev('t', { cmd: true }), 'linux')).toBe(false);
    expect(bindingMatchesEvent(b, ev('t', { ctrl: true }), 'linux')).toBe(false);
  });

  test('an extra modifier never matches', () => {
    const b = parseKeybinding('mod+t');
    expect(bindingMatchesEvent(b, ev('t', { cmd: true, alt: true }), 'darwin')).toBe(false);
  });

  test('plain Ctrl+T on Linux is terminal input, not a command', () => {
    expect(bindingMatchesEvent(parseKeybinding('mod+t'), ev('t', { ctrl: true }), 'linux')).toBe(
      false,
    );
  });

  test('key case and missing key are handled', () => {
    expect(bindingMatchesEvent(parseKeybinding('mod+t'), ev('T', { cmd: true }), 'darwin')).toBe(
      true,
    );
    expect(bindingMatchesEvent(parseKeybinding('mod+t'), { modifiers: {} }, 'darwin')).toBe(false);
  });

  test('missing modifiers object means no modifiers', () => {
    expect(bindingMatchesEvent(parseKeybinding('escape'), { key: 'escape' }, 'linux')).toBe(true);
  });
});

describe('keybindingEquals', () => {
  test('compares after resolution', () => {
    expect(keybindingEquals(parseKeybinding('mod+t'), parseKeybinding('cmd+t'), 'darwin')).toBe(
      true,
    );
    expect(
      keybindingEquals(parseKeybinding('mod+t'), parseKeybinding('ctrl+shift+t'), 'linux'),
    ).toBe(true);
    expect(keybindingEquals(parseKeybinding('mod+t'), parseKeybinding('mod+w'), 'linux')).toBe(
      false,
    );
  });
});

describe('detectPlatform', () => {
  test('macOS', () => {
    const p = detectPlatform({ platform: 'darwin', env: {}, execPath: '/usr/bin/bun' });
    expect(p).toMatchObject({ platform: 'darwin', isMac: true, isLinux: false, isWsl: false });
  });

  test('WSLg is detected from /proc/version and from the env', () => {
    expect(
      detectPlatform({
        platform: 'linux',
        env: {},
        execPath: '/usr/bin/bun',
        procVersion: 'Linux version 6.6.0-microsoft-standard-WSL2',
      }).isWsl,
    ).toBe(true);
    expect(
      detectPlatform({
        platform: 'linux',
        env: { WSL_DISTRO_NAME: 'Ubuntu' },
        execPath: '/usr/bin/bun',
        procVersion: 'Linux version 6.6.0-generic',
      }).isWsl,
    ).toBe(true);
    expect(
      detectPlatform({
        platform: 'linux',
        env: {},
        execPath: '/usr/bin/bun',
        procVersion: 'Linux version 6.6.0-generic',
      }).isWsl,
    ).toBe(false);
  });

  test('wayland and compiled detection', () => {
    const p = detectPlatform({
      platform: 'linux',
      env: { WAYLAND_DISPLAY: 'wayland-0' },
      execPath: '/opt/superterminal/superterminal',
      procVersion: '',
    });
    expect(p.isWayland).toBe(true);
    expect(p.isCompiled).toBe(true);
    expect(
      detectPlatform({ platform: 'linux', env: {}, execPath: '/usr/bin/bun', procVersion: '' })
        .isCompiled,
    ).toBe(false);
  });
});
