import { describe, expect, test } from 'bun:test';
import { DEFAULT_CONFIG, type Config, type WindowBackground } from '../config/schema.js';
import { buildTerminalTheme, tokensFor, glassTokens, opaqueTokens } from '../theme/tokens.js';
import { detectPlatform } from './detect.js';
import {
  buildWindowOptions,
  resolveBackground,
  resolveInitialSize,
  resolveLinuxBackground,
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
});
