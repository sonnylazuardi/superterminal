import { describe, expect, test } from 'bun:test';
import { DEFAULT_CONFIG, type Config, type WindowBackground } from '../config/schema.js';
import type { WindowPlacement } from '../state/client-state.js';
import { buildTerminalTheme, tokensFor, glassTokens, opaqueTokens } from '../theme/tokens.js';
import { detectPlatform } from './detect.js';
import {
  buildWindowOptions,
  resolveBackground,
  resolveInitialSize,
  resolveLinuxBackground,
  resolveWindowOrigin,
  titleBarPadding,
} from './window-options.js';

const withBackground = (background: WindowBackground): Config => ({
  ...DEFAULT_CONFIG,
  window: { ...DEFAULT_CONFIG.window, background },
});

const mac = detectPlatform({ platform: 'darwin', env: {}, execPath: '/usr/bin/bun' });
const wsl = detectPlatform({
  platform: 'linux',
  env: {},
  execPath: '/usr/bin/bun',
  procVersion: 'Linux 6.6.0-microsoft-standard-WSL2',
});
const wayland = detectPlatform({
  platform: 'linux',
  env: { WAYLAND_DISPLAY: 'wayland-0' },
  execPath: '/usr/bin/bun',
  procVersion: 'Linux 6.6.0-generic',
});
const x11 = detectPlatform({
  platform: 'linux',
  env: { DISPLAY: ':0' },
  execPath: '/usr/bin/bun',
  procVersion: 'Linux 6.6.0-generic',
});
const windows = detectPlatform({
  platform: 'win32',
  env: {},
  execPath: 'C:\\Users\\x\\superterminal.exe',
});

describe('resolveLinuxBackground', () => {
  test('auto: WSLg is opaque, Wayland transparent, X11 opaque', () => {
    expect(resolveLinuxBackground('auto', wsl)).toBe('opaque');
    expect(resolveLinuxBackground('auto', wayland)).toBe('transparent');
    expect(resolveLinuxBackground('auto', x11)).toBe('opaque');
  });

  test('an explicit value wins over detection', () => {
    expect(resolveLinuxBackground('transparent', wsl)).toBe('transparent');
    expect(resolveLinuxBackground('opaque', wayland)).toBe('opaque');
  });

  test('blurred degrades to transparent on Linux', () => {
    expect(resolveLinuxBackground('blurred', wayland)).toBe('transparent');
    expect(resolveLinuxBackground('blurred', wsl)).toBe('transparent');
  });
});

describe('resolveBackground on Windows', () => {
  test('auto and blurred are opaque; only explicit transparent stays transparent', () => {
    expect(resolveBackground(withBackground('auto'), windows)).toBe('opaque');
    expect(resolveBackground(withBackground('blurred'), windows)).toBe('opaque');
    expect(resolveBackground(withBackground('opaque'), windows)).toBe('opaque');
    expect(resolveBackground(withBackground('transparent'), windows)).toBe('transparent');
    // Not the macOS answer even though `auto` maps to blur there.
    expect(resolveBackground(withBackground('auto'), mac)).toBe('blurred');
  });
});

describe('buildWindowOptions', () => {
  test('macOS: blurred, transparent titlebar, traffic lights at 18/18', () => {
    const opts = buildWindowOptions(DEFAULT_CONFIG, mac);
    expect(opts).toMatchObject({
      title: 'superterminal',
      appName: 'superterminal',
      minWidth: 480,
      minHeight: 320,
      windowBackground: 'blurred',
      titlebarTransparent: true,
      trafficLightX: 18,
      trafficLightY: 13,
      focus: true,
    });
  });

  test('macOS honours an explicit background', () => {
    expect(resolveBackground(withBackground('opaque'), mac)).toBe('opaque');
  });

  test('Linux: no titlebar transparency and no traffic lights', () => {
    const opts = buildWindowOptions(DEFAULT_CONFIG, wayland);
    expect(opts.titlebarTransparent).toBe(false);
    expect(opts.trafficLightX).toBeUndefined();
    expect(opts.windowBackground).toBe('transparent');
  });

  test('window size from config is passed through when set', () => {
    const config: Config = {
      ...DEFAULT_CONFIG,
      window: { ...DEFAULT_CONFIG.window, width: 1400, height: 900 },
    };
    expect(buildWindowOptions(config, x11)).toMatchObject({ width: 1400, height: 900 });
    expect(buildWindowOptions(DEFAULT_CONFIG, x11).width).toBeUndefined();
  });

  test('a remembered size wins over config and is clamped to the minimum', () => {
    const config: Config = {
      ...DEFAULT_CONFIG,
      window: { ...DEFAULT_CONFIG.window, width: 1400, height: 900 },
    };
    expect(buildWindowOptions(config, x11, { width: 1017, height: 655 })).toMatchObject({
      width: 1017,
      height: 655,
    });
    expect(resolveInitialSize(config, { width: 100, height: 100 })).toEqual({
      width: 480,
      height: 320,
    });
    expect(resolveInitialSize(config, { width: 1017.6, height: 655.2 })).toEqual({
      width: 1018,
      height: 655,
    });
    expect(resolveInitialSize(DEFAULT_CONFIG, null)).toBeNull();
    // Config needs both dimensions; one alone is not a size.
    const halfConfig: Config = { ...DEFAULT_CONFIG, window: { ...DEFAULT_CONFIG.window, width: 1400 } };
    expect(resolveInitialSize(halfConfig, null)).toBeNull();
  });

  test('a remembered placement adds origin, display and maximized', () => {
    const placement: WindowPlacement = {
      width: 1017,
      height: 655,
      x: 1920,
      y: 120,
      maximized: true,
      display: { uuid: 'U', bounds: { x: 1920, y: 0, width: 1920, height: 1080 } },
    };
    expect(buildWindowOptions(DEFAULT_CONFIG, windows, placement)).toMatchObject({
      width: 1017,
      height: 655,
      x: 1920,
      y: 120,
      maximized: true,
      display: { uuid: 'U', bounds: { x: 1920, y: 0, width: 1920, height: 1080 } },
    });
  });

  test('an origin whose remembered display no longer contains the centre is dropped', () => {
    const stale: WindowPlacement = {
      width: 1017,
      height: 655,
      x: 4000,
      y: 4000,
      display: { bounds: { x: 0, y: 0, width: 1920, height: 1080 } },
    };
    expect(resolveWindowOrigin(stale)).toBeNull();
    const opts = buildWindowOptions(DEFAULT_CONFIG, windows, stale);
    expect(opts.x).toBeUndefined();
    expect(opts.y).toBeUndefined();
    expect(opts).toMatchObject({ width: 1017, height: 655 });
  });

  test('a uuid keeps the origin even when the remembered bounds moved', () => {
    const moved: WindowPlacement = {
      width: 1017,
      height: 655,
      x: 3000,
      y: 4000,
      display: { uuid: 'U', bounds: { x: 0, y: 0, width: 1920, height: 1080 } },
    };
    expect(resolveWindowOrigin(moved)).toEqual({ x: 3000, y: 4000 });
    expect(buildWindowOptions(DEFAULT_CONFIG, windows, moved)).toMatchObject({ x: 3000, y: 4000 });
  });

  test('maximized is passed through only when true', () => {
    expect(
      buildWindowOptions(DEFAULT_CONFIG, windows, { width: 800, height: 600 }).maximized,
    ).toBeUndefined();
    expect(
      buildWindowOptions(DEFAULT_CONFIG, windows, {
        width: 800,
        height: 600,
        maximized: false,
      }).maximized,
    ).toBeUndefined();
  });

  test('title bar padding is macOS-only', () => {
    expect(titleBarPadding(mac, 58)).toBe(58);
    expect(titleBarPadding(x11, 58)).toBe(0);
  });
});

describe('tokens', () => {
  test('opaque windows get real colours instead of alpha white', () => {
    expect(tokensFor('opaque')).toBe(opaqueTokens);
    expect(tokensFor('blurred')).toBe(glassTokens);
    expect(tokensFor('transparent')).toBe(glassTokens);
    expect(opaqueTokens.bg.glass).toBe('#1E1E22');
    expect(glassTokens.bg.glass).toBe('#FFFFFF0D');
    // Shared tokens are identical across both palettes.
    expect(opaqueTokens.fg).toEqual(glassTokens.fg);
    expect(opaqueTokens.accent).toBe(glassTokens.accent);
  });
});

describe('buildTerminalTheme', () => {
  test('defaults to the neutral dark palette', () => {
    const theme = buildTerminalTheme();
    expect(theme.ansi).toHaveLength(16);
    expect(theme.bg).toBe('#1e1e1e');
    expect(theme.fg).toBe('#d4d4d4');
    expect(theme.boldIsBright).toBe(false);
    expect(theme.selectionFg).toBeUndefined();
  });

  test('applies config overrides in both spellings', () => {
    const theme = buildTerminalTheme(
      { bg: '#000000', ansi1: '#ff0000', cursor_text: '#111111', selectionFg: '#ffffff' },
      true,
    );
    expect(theme.bg).toBe('#000000');
    expect(theme.ansi[1]).toBe('#ff0000');
    expect(theme.ansi[2]).toBe('#0dbc79');
    expect(theme.cursorText).toBe('#111111');
    expect(theme.selectionFg).toBe('#ffffff');
    expect(theme.boldIsBright).toBe(true);
  });

  test('the documented [theme] keys from config-example.toml reach the cells', () => {
    // Exactly what a user copies out of docs/config-example.toml (and what
    // st-config's ThemeConfig deserialises for OSC 10/11).
    const theme = buildTerminalTheme({
      foreground: '#DCDCDC',
      background: '#15191F',
      cursor: '#FFFFFF',
      cursor_text: '#000000',
      selection_background: '#B3D7FF',
      selection_foreground: '#000000',
      black: '#14191E',
      red: '#B43C2A',
      green: '#00C200',
      yellow: '#C7C400',
      blue: '#2744C7',
      magenta: '#C040BE',
      cyan: '#00C5C7',
      white: '#C7C7C7',
      bright_black: '#686868',
      bright_red: '#DD7975',
      bright_green: '#58E790',
      bright_yellow: '#ECE100',
      bright_blue: '#A7ABF2',
      bright_magenta: '#E17EE1',
      bright_cyan: '#60FDFF',
      bright_white: '#FFFFFF',
    });
    expect(theme.fg).toBe('#DCDCDC');
    expect(theme.bg).toBe('#15191F');
    expect(theme.cursorText).toBe('#000000');
    expect(theme.selectionBg).toBe('#B3D7FF');
    expect(theme.selectionFg).toBe('#000000');
    expect(theme.ansi).toEqual([
      '#14191E', '#B43C2A', '#00C200', '#C7C400', '#2744C7', '#C040BE', '#00C5C7', '#C7C7C7',
      '#686868', '#DD7975', '#58E790', '#ECE100', '#A7ABF2', '#E17EE1', '#60FDFF', '#FFFFFF',
    ]);
  });

  test('ansiN wins over the named key when both are given', () => {
    const theme = buildTerminalTheme({ ansi1: '#111111', red: '#222222', bright_red: '#333333' });
    expect(theme.ansi[1]).toBe('#111111');
    expect(theme.ansi[9]).toBe('#333333');
  });
});
