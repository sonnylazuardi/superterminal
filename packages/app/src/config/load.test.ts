import { describe, expect, test } from 'bun:test';
import { configPath, loadConfig, parseConfig, parseConfigText } from './load.js';
import { DEFAULT_CONFIG } from './schema.js';

describe('configPath', () => {
  test('respects XDG_CONFIG_HOME, then HOME, then an explicit override', () => {
    expect(configPath({ XDG_CONFIG_HOME: '/xdg', HOME: '/home/x' })).toBe(
      '/xdg/superterminal/config.toml',
    );
    expect(configPath({ HOME: '/home/x' })).toBe('/home/x/.config/superterminal/config.toml');
    expect(configPath({ SUPERTERMINAL_CONFIG: '/tmp/c.toml', HOME: '/home/x' })).toBe(
      '/tmp/c.toml',
    );
  });
});

describe('defaults', () => {
  test('a missing file yields defaults with no warnings', () => {
    const result = loadConfig({ path: '/nope/config.toml', exists: () => false });
    expect(result.config).toEqual(DEFAULT_CONFIG);
    expect(result.path).toBeNull();
    expect(result.warnings).toEqual([]);
  });

  test('the default config matches the documented values', () => {
    expect(DEFAULT_CONFIG.font).toEqual({ size: 13, lineHeight: 1.2 });
    expect(DEFAULT_CONFIG.window).toEqual({ background: 'auto', verticalTabs: false });
    expect(DEFAULT_CONFIG.terminal.scrollbackLines).toBe(10_000);
    expect(DEFAULT_CONFIG.terminal.boldIsBright).toBe(false);
    expect(DEFAULT_CONFIG.theme).toEqual({});
    expect(DEFAULT_CONFIG.keybindings).toEqual({});
  });

  test('an empty file yields defaults', () => {
    expect(parseConfigText('').config).toEqual(DEFAULT_CONFIG);
  });
});

describe('parsing a real file', () => {
  const toml = `
[font]
family = "Berkeley Mono"
size = 15
line_height = 1.35

[window]
background = "transparent"
vertical_tabs = true

[shell]
program = "/bin/fish"
args = ["-l"]

[terminal]
scrollback_lines = 50000
bold_is_bright = true
cursor_style = "beam"

[theme]
bg = "#101014"
ansi0 = "#000000"

[keybindings]
"tab.new" = "mod+shift+t"
"app.quit" = ""
`;

  test('reads every table', () => {
    const { config, warnings } = parseConfigText(toml);
    expect(warnings).toEqual([]);
    expect(config.font).toEqual({ family: 'Berkeley Mono', size: 15, lineHeight: 1.35 });
    expect(config.window.background).toBe('transparent');
    expect(config.window.verticalTabs).toBe(true);
    expect(config.shell.program).toBe('/bin/fish');
    expect(config.shell.args).toEqual(['-l']);
    expect(config.terminal).toEqual({
      scrollbackLines: 50_000,
      boldIsBright: true,
      cursorStyle: 'beam',
      cursorBlink: true,
      scrollbar: 'auto',
    });
    expect(config.theme).toEqual({ bg: '#101014', ansi0: '#000000' });
    expect(config.keybindings).toEqual({ 'tab.new': 'mod+shift+t', 'app.quit': '' });
  });

  test('camelCase spellings are accepted as well as snake_case', () => {
    const { config, warnings } = parseConfigText(
      '[font]\nlineHeight = 1.5\n[window]\nverticalTabs = true\n[terminal]\nscrollbackLines = 42\n',
    );
    expect(warnings).toEqual([]);
    expect(config.font.lineHeight).toBe(1.5);
    expect(config.window.verticalTabs).toBe(true);
    expect(config.terminal.scrollbackLines).toBe(42);
  });

  test('partial tables keep their defaults', () => {
    const { config } = parseConfigText('[font]\nsize = 16\n');
    expect(config.font).toEqual({ size: 16, lineHeight: 1.2 });
    expect(config.window).toEqual(DEFAULT_CONFIG.window);
  });

  test('loadConfig reads through the injected filesystem', () => {
    const result = loadConfig({
      path: '/etc/st/config.toml',
      exists: () => true,
      readFile: () => '[font]\nsize = 11\n',
    });
    expect(result.path).toBe('/etc/st/config.toml');
    expect(result.config.font.size).toBe(11);
  });

  test('an unreadable file falls back to defaults with a warning', () => {
    const result = loadConfig({
      path: '/etc/st/config.toml',
      exists: () => true,
      readFile: () => {
        throw new Error('EACCES');
      },
    });
    expect(result.config).toEqual(DEFAULT_CONFIG);
    expect(result.warnings[0]).toContain('could not read');
  });
});

describe('malformed input', () => {
  test('a TOML syntax error warns and yields defaults', () => {
    const { config, warnings } = parseConfigText('[font\nsize = ');
    expect(config).toEqual(DEFAULT_CONFIG);
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toContain('not valid TOML');
  });

  test('unknown tables and keys warn and are ignored', () => {
    const { config, warnings } = parseConfigText(
      '[nonsense]\nx = 1\n[font]\nsize = 14\nweight = "bold"\n',
    );
    expect(config.font.size).toBe(14);
    expect(warnings).toEqual([
      '[superterminal] unknown config key ignored: nonsense',
      '[superterminal] unknown config key ignored: font.weight',
    ]);
  });

  test('the server-owned [server] table is neither read nor warned about', () => {
    const { warnings } = parseConfigText('[server]\nidle_exit_minutes = 30\n');
    expect(warnings).toEqual([]);
  });

  test('a wrongly typed value warns and that table falls back', () => {
    const { config, warnings } = parseConfigText('[font]\nsize = "big"\n[window]\nverticalTabs = true\n');
    expect(warnings.some((w) => w.includes('invalid config at font.size'))).toBe(true);
    expect(config.font).toEqual(DEFAULT_CONFIG.font);
    // A good table next to a bad one survives.
    expect(config.window.verticalTabs).toBe(true);
  });

  test('an out-of-range enum warns', () => {
    const { config, warnings } = parseConfigText('[window]\nbackground = "rainbow"\n');
    expect(warnings.some((w) => w.includes('invalid config at window.background'))).toBe(true);
    expect(config.window.background).toBe('auto');
  });

  test('a negative font size is rejected', () => {
    const { config, warnings } = parseConfigText('[font]\nsize = -3\n');
    expect(warnings.length).toBeGreaterThan(0);
    expect(config.font.size).toBe(13);
  });

  test('scrollback is capped at 100000', () => {
    const { config, warnings } = parseConfigText('[terminal]\nscrollback_lines = 1000000\n');
    expect(warnings.length).toBeGreaterThan(0);
    expect(config.terminal.scrollbackLines).toBe(10_000);
  });

  test('a non-object root is handled', () => {
    expect(parseConfig(null).config).toEqual(DEFAULT_CONFIG);
    expect(parseConfig(42).config).toEqual(DEFAULT_CONFIG);
    expect(parseConfig('hello').config).toEqual(DEFAULT_CONFIG);
  });
});
