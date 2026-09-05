import { describe, expect, test } from 'bun:test';
import { displayTitle } from './title.js';

describe('displayTitle', () => {
  test('drops the user@host: prefix a shell puts in the OSC title', () => {
    expect(displayTitle('sonny@LAPTOP-RAZER-SONNY:~', 'shell')).toBe('~');
    expect(displayTitle('sonny@box:~/projects/superterminal', 'shell')).toBe(
      '~/projects/superterminal',
    );
    expect(displayTitle('root@host:/', 'shell')).toBe('/');
  });

  test('leaves other titles alone', () => {
    expect(displayTitle('OpenCode', 'shell')).toBe('OpenCode');
    expect(displayTitle('vim: notes.md', 'shell')).toBe('vim: notes.md');
    expect(displayTitle('mail@example.com', 'shell')).toBe('mail@example.com');
    expect(displayTitle('OC | Assistant introduction', 'shell')).toBe('OC | Assistant introduction');
  });

  test('empty or prefix-only titles fall back', () => {
    expect(displayTitle('', 'shell')).toBe('shell');
    expect(displayTitle(undefined, 'superterminal')).toBe('superterminal');
    expect(displayTitle('sonny@host:', 'shell')).toBe('shell');
  });
});
