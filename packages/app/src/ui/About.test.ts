import { describe, expect, test } from 'bun:test';
import { aboutLines, displayUrl } from './About.js';

describe('About', () => {
  test('a dev build shows only the version', () => {
    expect(aboutLines({ version: '0.1.14', buildId: 'dev' })).toEqual(['Version 0.1.14']);
    expect(aboutLines({ version: '0.1.14', buildId: '0.1.14' })).toEqual(['Version 0.1.14']);
  });

  test('a packaged build adds its build id', () => {
    expect(aboutLines({ version: '0.1.14', buildId: '0.1.14+abc123' })).toEqual([
      'Version 0.1.14',
      'Build 0.1.14+abc123',
    ]);
  });

  test('the link is shown without its scheme', () => {
    expect(displayUrl('https://github.com/sonnylazuardi/superterminal')).toBe(
      'github.com/sonnylazuardi/superterminal',
    );
  });
});
