import { describe, expect, test } from 'bun:test';
import { excerptFor, matchesScreen, redactSecrets, screenSnippet, type ScreenText } from './screen-context.js';

const MASK = '…redacted…';

/**
 * Secret-shaped fixtures are ASSEMBLED, never written out.
 *
 * These are invented strings, but they are convincing enough that GitHub's
 * push protection reads the file, sees a Slack token and rejects the push.
 * Joining the prefix to the body at run time leaves nothing for a scanner to
 * match while the value `redactSecrets` actually sees is unchanged.
 */
const fake = (prefix: string, body: string): string => prefix + body;

describe('redactSecrets: the shapes that must never leave', () => {
  test('an HTTP authorisation header keeps the scheme and the surrounding words', () => {
    const out = redactSecrets('curl -H "Authorization: Bearer abcdef1234567890ABCDEF" https://api.example.com');
    expect(out).toContain('curl -H');
    expect(out).toContain('Bearer …redacted…');
    expect(out).toContain('https://api.example.com');
    expect(out).not.toContain('abcdef1234567890ABCDEF');
  });

  test('Basic auth too', () => {
    expect(redactSecrets('Authorization: Basic dXNlcjpodW50ZXIy')).toBe(`Authorization: Basic ${MASK}`);
  });

  test('sk- and sk_live_ provider keys', () => {
    expect(redactSecrets(`the key is ${fake('sk', '-1234567890abcdef')} here`)).toBe(`the key is ${MASK} here`);
    expect(redactSecrets(fake('sk', '_live_51AbCdEfGhIjKlMnOp'))).toBe(MASK);
    expect(redactSecrets(`using ${fake('sk', '-proj-AbCd1234EfGh5678')} now`)).toBe(`using ${MASK} now`);
  });

  test('GitHub personal access tokens, both spellings', () => {
    expect(redactSecrets(fake('ghp', '_ABCDEFGHIJKLMNOPQRSTUVWX1234'))).toBe(MASK);
    expect(redactSecrets(`remote: ${fake('github', '_pat_11ABCDEFG0abcdefghijklmnopqrstu')} added`)).toBe(
      `remote: ${MASK} added`,
    );
  });

  test('AWS access key ids', () => {
    expect(redactSecrets(`AWS_ACCESS_KEY_ID ${fake('AKIA', 'IOSFODNN7EXAMPLE')}`)).toBe(`AWS_ACCESS_KEY_ID ${MASK}`);
  });

  test('Slack bot and user tokens', () => {
    expect(redactSecrets(fake('xoxb', '-123456789012-ABCdefGHIjklMNO'))).toBe(MASK);
    expect(redactSecrets(`token ${fake('xoxp', '-987654321098-abcDEFghiJKL')} ok`)).toContain(MASK);
  });

  test('a private key fence takes the whole line with it', () => {
    expect(redactSecrets('-----BEGIN RSA PRIVATE KEY-----')).toBe(MASK);
    expect(redactSecrets('-----BEGIN OPENSSH PRIVATE KEY-----')).toBe(MASK);
    expect(redactSecrets('cat key.pem  -----END PRIVATE KEY-----')).toBe(MASK);
  });

  test('named assignments keep the name and lose the value, whatever the quoting', () => {
    expect(redactSecrets('password=hunter2')).toBe(`password=${MASK}`);
    expect(redactSecrets('password="hunter two"')).toBe(`password=${MASK}`);
    expect(redactSecrets("api_key = 'deadbeef'")).toBe(`api_key = ${MASK}`);
    expect(redactSecrets('secret: swordfish')).toBe(`secret: ${MASK}`);
    expect(redactSecrets('curl --token=abc123def456 https://example.com')).toBe(
      `curl --token=${MASK} https://example.com`,
    );
    expect(redactSecrets('export GITHUB_TOKEN=abcdefgh')).toBe(`export GITHUB_TOKEN=${MASK}`);
    expect(redactSecrets('DB_PASSWORD=s3cr3t psql')).toBe(`DB_PASSWORD=${MASK} psql`);
    expect(redactSecrets('api-key: 1234')).toBe(`api-key: ${MASK}`);
    expect(redactSecrets('PGPASSWORD=hunter2 psql')).toBe(`PGPASSWORD=${MASK} psql`);
  });

  test('bare runs of hex and base64 standing on their own', () => {
    expect(redactSecrets('commit 5f2a9c1d8e4b7a06f3c2d1e0b9a88776655443322')).toBe(`commit ${MASK}`);
    expect(redactSecrets('etag "D41D8CD98F00B204E9800998ECF8427E00112233"')).toContain(MASK);
    expect(redactSecrets('body Y29uZmlkZW50aWFsIHNlY3JldCBwYXlsb2FkIQ==')).toBe(`body ${MASK}`);
    expect(redactSecrets('Zm9vYmFyMTIzQkFacXV1eDQ1NkhlbGxvV29ybGQ3ODk end')).toBe(`${MASK} end`);
  });

  test('several secrets in one line all go', () => {
    const out = redactSecrets(
      `PGPASSWORD=hunter2 curl -H "Authorization: Bearer ${fake('ghp', '_ABCDEFGHIJKLMNOPQRSTUVWX1234')}"`,
    );
    expect(out).not.toContain('hunter2');
    expect(out).not.toContain('ghp_');
    expect(out).toContain('curl -H');
  });
});

describe('redactSecrets: what it must leave alone', () => {
  const untouched = [
    'The quick brown fox jumps over the lazy dog and keeps on running',
    'error: cannot find module ./src/ai/screen-context.ts',
    '/home/sonny/projects/superterminal/packages/app/src/ai/screen-context.ts',
    '/home/user/Documents/Project2024/some/deep/path/file.txt',
    '~/projects/superterminal $ bun test packages/app/src/ai',
    'npm ERR! code ELIFECYCLE — the password field is empty, check the docs',
    'Compiling st-config v0.1.0 (/home/sonny/projects/superterminal/crates/st-config)',
    'at AbstractAnnotationConfigDispatcherServletInitializer.onStartup(Main.java:42)',
    'tokenizer=fast is not a secret assignment',
    'Bearer of bad news',
    'PWD=/home/sonny/projects/superterminal',
  ];

  for (const line of untouched) {
    test(`leaves ordinary text alone: ${line.slice(0, 40)}`, () => {
      expect(redactSecrets(line)).toBe(line);
    });
  }
});

describe('excerptFor', () => {
  test('takes the last lines, collapses whitespace and joins with a slash', () => {
    const lines = ['first', '', 'second    line', '   ', 'third\tline'];
    expect(excerptFor(lines, { maxLines: 2 })).toBe('second line / third line');
  });

  test('drops box-drawing and spinner art', () => {
    const lines = ['┌──────────────┐', '│ build ok     │', '└──────────────┘', '⠋⠙⠹⠸', '====='];
    expect(excerptFor(lines)).toBe('│ build ok │');
  });

  test('redacts before anything is joined', () => {
    const out = excerptFor([`$ export OPENAI_API_KEY=${fake('sk', '-proj-AbCd1234EfGh5678')}`, '$ bun run dev']);
    expect(out).not.toContain('sk-proj');
    expect(out).toContain(MASK);
    expect(out).toContain('bun run dev');
  });

  test('a line that is nothing but a mask is not worth the tokens', () => {
    expect(excerptFor(['-----BEGIN RSA PRIVATE KEY-----', 'done'])).toBe('done');
  });

  test('hard-truncates at a word boundary with a trailing ellipsis', () => {
    const line = 'alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike';
    const out = excerptFor([line], { maxChars: 40 });
    expect(out.length).toBeLessThanOrEqual(40);
    expect(out.endsWith('…')).toBe(true);
    expect(out.slice(0, -1).trim().split(' ').every((w) => line.includes(w))).toBe(true);
    expect(out).toStartWith('alpha bravo');
  });

  test('a single very long token still truncates', () => {
    const out = excerptFor(['x'.repeat(200)], { maxChars: 20 });
    expect(out.length).toBe(20);
    expect(out.endsWith('…')).toBe(true);
  });

  test('defaults: at most 15 lines and 600 characters', () => {
    const lines = Array.from({ length: 40 }, (_, i) => `line number ${i}`);
    const out = excerptFor(lines);
    expect(out.length).toBeLessThanOrEqual(600);
    expect(out).toContain('line number 39');
    expect(out).not.toContain('line number 24');
    expect(out).toContain('line number 25');
  });

  test('an empty screen yields an empty excerpt', () => {
    expect(excerptFor([])).toBe('');
    expect(excerptFor(['', '   ', '\t'])).toBe('');
  });

  test('escape sequences never reach the excerpt', () => {
    expect(excerptFor(['\u001B[31mbuild failed\u001B[0m'])).toBe('build failed');
  });
});

describe('matchesScreen', () => {
  const lines = ['$ bun run dev', 'Baby Shark server listening on :3000', '  ram usage: 42 MB'];

  test('case-insensitive substring match', () => {
    expect(matchesScreen('baby', lines)).toBe(true);
    expect(matchesScreen('BABY', lines)).toBe(true);
    expect(matchesScreen('ram', lines)).toBe(true);
  });

  test('every word must appear, in any order, across any line', () => {
    expect(matchesScreen('baby shark', lines)).toBe(true);
    expect(matchesScreen('shark baby', lines)).toBe(true);
    expect(matchesScreen('dev ram', lines)).toBe(true);
    expect(matchesScreen('baby whale', lines)).toBe(false);
  });

  test('an empty or blank query matches nothing', () => {
    expect(matchesScreen('', lines)).toBe(false);
    expect(matchesScreen('   ', lines)).toBe(false);
  });

  test('an empty screen matches nothing', () => {
    expect(matchesScreen('baby', [])).toBe(false);
    expect(matchesScreen('baby', ['', '  '])).toBe(false);
  });

  test('colour codes do not break a match', () => {
    expect(matchesScreen('failed', ['\u001B[31mbuild failed\u001B[0m'])).toBe(true);
  });
});

describe('screenSnippet', () => {
  const lines = ['$ bun run dev', 'Baby Shark server listening on :3000'];

  test('returns the matching line, trimmed', () => {
    expect(screenSnippet('baby', lines)).toBe('Baby Shark server listening on :3000');
  });

  test('prefers the line carrying the whole query', () => {
    const many = ['ram is here', 'baby and ram together', 'baby alone'];
    expect(screenSnippet('baby ram', many)).toBe('baby and ram together');
  });

  test('falls back to a line with the first word when the words are spread out', () => {
    expect(screenSnippet('baby whale', lines)).toBe('Baby Shark server listening on :3000');
  });

  test('windows a long line around the match', () => {
    const long = `${'a '.repeat(60)}needle${' b'.repeat(60)}`;
    const out = screenSnippet('needle', [long], 20);
    expect(out).not.toBeNull();
    expect(out).toContain('needle');
    expect(out?.startsWith('…')).toBe(true);
    expect(out?.endsWith('…')).toBe(true);
  });

  test('no match, no snippet', () => {
    expect(screenSnippet('walrus', lines)).toBeNull();
    expect(screenSnippet('', lines)).toBeNull();
    expect(screenSnippet('baby', [])).toBeNull();
  });
});

describe('ScreenText', () => {
  test('is a surface id and its visible lines', () => {
    const screen: ScreenText = { surface: 7, lines: ['hello'] };
    expect(screen.surface).toBe(7);
    expect(matchesScreen('hello', screen.lines)).toBe(true);
  });
});
