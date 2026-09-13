import { describe, expect, test } from 'bun:test';
import { APP_VERSION, describeBuild, resolveBuildInfo } from './version.js';

describe('version', () => {
  test('the checked-in release number is what ships when the environment says nothing', () => {
    expect(resolveBuildInfo({})).toEqual({ version: APP_VERSION, buildId: 'dev' });
    expect(resolveBuildInfo({ SUPERTERMINAL_VERSION: '  ', SUPERTERMINAL_BUILD_ID: '' })).toEqual({
      version: APP_VERSION,
      buildId: 'dev',
    });
  });

  test('the environment overrides both fields', () => {
    expect(
      resolveBuildInfo({ SUPERTERMINAL_VERSION: '9.9.9', SUPERTERMINAL_BUILD_ID: '9.9.9+abc123' }),
    ).toEqual({ version: '9.9.9', buildId: '9.9.9+abc123' });
  });

  test('describeBuild mentions the build id only when it adds something', () => {
    expect(describeBuild({ version: '0.1.14', buildId: 'dev' })).toBe('superterminal 0.1.14');
    expect(describeBuild({ version: '0.1.14', buildId: '0.1.14' })).toBe('superterminal 0.1.14');
    expect(describeBuild({ version: '0.1.14', buildId: '0.1.14+abc123' })).toBe(
      'superterminal 0.1.14 (0.1.14+abc123)',
    );
  });

  test('APP_VERSION looks like a release number', () => {
    expect(APP_VERSION).toMatch(/^\d+\.\d+\.\d+$/);
  });
});
