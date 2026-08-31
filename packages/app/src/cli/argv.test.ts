import { describe, expect, test } from 'bun:test';
import { parseArgv } from './argv.js';

describe('parseArgv', () => {
  test('defaults', () => {
    expect(parseArgv([])).toEqual({
      version: false,
      help: false,
      noSpawn: false,
      foregroundServer: false,
      unknown: [],
    });
  });

  test('flags', () => {
    const argv = parseArgv(['--no-spawn', '--foreground-server', '-v', '-h']);
    expect(argv).toMatchObject({
      noSpawn: true,
      foregroundServer: true,
      version: true,
      help: true,
    });
  });

  test('value flags accept both spellings', () => {
    expect(parseArgv(['--socket', '/tmp/a.sock']).socket).toBe('/tmp/a.sock');
    expect(parseArgv(['--socket=/tmp/b.sock']).socket).toBe('/tmp/b.sock');
    expect(parseArgv(['--config=/etc/c.toml']).config).toBe('/etc/c.toml');
  });

  test('unknown args are collected, not fatal', () => {
    expect(parseArgv(['--wat', 'positional']).unknown).toEqual(['--wat', 'positional']);
  });
});
