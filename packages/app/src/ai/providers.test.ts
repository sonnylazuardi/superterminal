import { describe, expect, test } from 'bun:test';
import { detectProvider, PROVIDER_ORDER, PROVIDERS, providerLabel, resolveTarget } from './providers.js';

describe('provider presets', () => {
  test('TypeSafe direct pins the versioned model', () => {
    expect(PROVIDERS.typesafe).toEqual({
      id: 'typesafe',
      label: 'TypeSafe',
      endpoint: 'https://api.typesafe.ai/v1/systemone',
      model: 'jev-1.13.0',
      envKey: 'TYPESAFE_API_KEY',
      opencodeLogin: false,
      keyPrefix: 'apikey_',
      consoleUrl: 'https://console.typesafe.ai/keys',
    });
  });

  test('Zen keeps its own model id and the OpenCode login', () => {
    expect(PROVIDERS.zen).toMatchObject({
      endpoint: 'https://opencode.ai/zen/v1/systemone',
      model: 'jev-1.13',
      envKey: null,
      opencodeLogin: true,
      keyPrefix: null,
    });
  });

  test('auto prefers the direct endpoint', () => {
    expect(PROVIDER_ORDER).toEqual(['typesafe', 'zen']);
  });

  test('labels', () => {
    expect(providerLabel('typesafe')).toBe('TypeSafe');
    expect(providerLabel('zen')).toBe('OpenCode Zen');
    expect(providerLabel(null)).toBe('none');
  });
});

describe('detectProvider', () => {
  test('apikey_ keys are TypeSafe, anything else Zen', () => {
    expect(detectProvider('apikey_abc123_def456')).toBe('typesafe');
    expect(detectProvider('  apikey_abc  ')).toBe('typesafe');
    expect(detectProvider('sk-zen-abcdef')).toBe('zen');
    expect(detectProvider('APIKEY_abc')).toBe('zen');
  });
});

describe('resolveTarget', () => {
  test('defaults to the preset', () => {
    expect(resolveTarget('typesafe', {})).toEqual({ endpoint: PROVIDERS.typesafe.endpoint, model: 'jev-1.13.0' });
    expect(resolveTarget('zen', {})).toEqual({ endpoint: PROVIDERS.zen.endpoint, model: 'jev-1.13' });
  });

  test('config overrides either field independently; blanks do not count', () => {
    expect(resolveTarget('typesafe', { model: 'jev-latest' })).toEqual({
      endpoint: PROVIDERS.typesafe.endpoint,
      model: 'jev-latest',
    });
    expect(resolveTarget('zen', { endpoint: 'https://proxy.test/systemone', model: '  ' })).toEqual({
      endpoint: 'https://proxy.test/systemone',
      model: 'jev-1.13',
    });
  });
});
