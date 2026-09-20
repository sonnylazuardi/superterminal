/**
 * Screen text for the palette: the offline match, and the one excerpt a
 * provider is ever allowed to see (08 §H, "screen-content tab search").
 *
 * WHY this lives on its own, pure and dependency-free: it is the last gate
 * before the contents of your terminal leave the machine. Nothing else in the
 * client may assemble screen text for a request — everything a model is shown
 * goes through `excerptFor`, and `excerptFor` goes through `redactSecrets`.
 * Being pure means the gate can be tested exhaustively with no network, no
 * GPU and no Server, which is the only way to believe a privacy claim.
 *
 * The other half of the file never leaves the machine at all. `matchesScreen`
 * and `screenSnippet` are plain, case-insensitive substring work so that
 * typing "baby" jumps to the tab showing it with no key configured, no
 * network and no provider — the model is an addition to that, never a
 * prerequisite.
 *
 * Redaction is deliberately conservative: it masks shapes that are secrets
 * and very little else, because a false positive in ordinary prose or in a
 * file path silently makes the excerpt useless, while a false negative is a
 * leak. Where the two pull against each other, the shape has to be
 * distinctive (a known prefix, a named assignment, a long random-looking run
 * standing on its own) before anything is masked.
 */

/** One tab's visible screen, as the Server hands it over. */
export interface ScreenText {
  surface: number;
  lines: string[];
}

/** What replaces the secret part. Short, and obviously not the original. */
const REDACTED = '…redacted…';

/**
 * CSI, OSC and two-byte escape sequences. Screen text arrives with its colours
 * and title-setting still in it; none of that is worth a token, and an OSC
 * payload can carry a path or a command line we would rather not send.
 */
const ANSI =
  // eslint-disable-next-line no-control-regex
  /\u001B\[[0-?]*[ -/]*[@-~]|\u001B\][\s\S]*?(?:\u0007|\u001B\\)|\u001B[@-Z\\-_]/g;

/**
 * A private-key fence. The body around it is key material, so a line carrying
 * the marker is dropped wholesale rather than picked apart.
 */
const PRIVATE_KEY = /-----\s*(?:BEGIN|END)[A-Za-z0-9 ]*PRIVATE KEY\s*-----/i;

/**
 * `password=…`, `GITHUB_TOKEN: …`, `--api-key "…"`. The name is kept so the
 * line still reads; the value never is. A free prefix catches the usual
 * environment-variable spellings (`PGPASSWORD`, `MY_GITHUB_TOKEN`), while the
 * name itself may not run on — `tokenizer=fast` is not a token — which is
 * what keeps ordinary text out of here. `pwd` is deliberately absent: `PWD=`
 * in `env` output is a working directory, not a password.
 */
const ASSIGNMENT =
  /(^|[^A-Za-z0-9_])([A-Za-z0-9_]*(?:password|passwd|token|api[-_]?key|secret|access[-_]?key)s?)(\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\s,;)]+)/gi;

/**
 * HTTP authorisation headers as they appear in curl output and logs. Matched
 * case-sensitively, since the lower-case word is ordinary English.
 */
const AUTH_SCHEME = /\b(Bearer|Basic)\s+[A-Za-z0-9._~+/=-]{8,}/g;

/**
 * Tokens that announce themselves with a vendor prefix: OpenAI/Stripe-style
 * `sk-` and `sk_live_`, GitHub `ghp_`/`github_pat_`, AWS access-key ids and
 * Slack bot/user tokens. A prefix plus a long body is distinctive enough that
 * a false positive needs bad luck.
 */
const PREFIXED_TOKEN = new RegExp(
  [
    String.raw`\b(?:sk|pk|rk)[-_](?:live|test|proj|prod)?[-_]?[A-Za-z0-9]{8,}`,
    String.raw`\bgithub_pat_[A-Za-z0-9_]{20,}`,
    String.raw`\bgh[pousr]_[A-Za-z0-9]{16,}`,
    String.raw`\b(?:AKIA|ASIA)[0-9A-Z]{16}\b`,
    String.raw`\bxox[baprse]-[A-Za-z0-9-]{10,}`,
  ].join('|'),
  'g',
);

/**
 * A bare run of 32+ hex characters standing on its own — a hash, a session id,
 * a raw key. The leading group keeps us out of paths: a run preceded by `/`
 * or `\` or a letter is part of something else and is left alone.
 */
const BARE_HEX = /(^|[\s"'=:(\[,])((?:[0-9a-f]{32,}|[0-9A-F]{32,}))(?=$|[\s"'),.;\]])/g;

/**
 * The same, for base64-looking runs. `-` and `_` are excluded from the body on
 * purpose: they would swallow long-hyphenated words and snake_case identifiers,
 * and the vendor prefixes above already cover the tokens that use them.
 */
const BARE_BASE64 = /(^|[\s"'=:(\[,])([A-Za-z0-9+/]{32,}={0,2})(?=$|[\s"'),.;\]])/g;

/**
 * Does a long run look like key material rather than something ordinary?
 * Base64 padding is decisive on its own; otherwise we want mixed case *and*
 * two digits, which a long camelCase identifier or a Java class name in a
 * stack trace almost never has.
 */
function looksRandom(text: string): boolean {
  if (text.endsWith('=')) return true;
  const digits = text.replace(/[^0-9]/g, '').length;
  return /[a-z]/.test(text) && /[A-Z]/.test(text) && digits >= 2;
}

/**
 * `/` is a legal base64 character and also a path separator, so a long path
 * can look exactly like a blob. Paths give themselves away by having several
 * segments that are plain lower-case words; random bodies do not.
 */
function looksLikePath(text: string): boolean {
  const segments = text.split('/');
  if (segments.length < 3) return false;
  const wordish = segments.filter((s) => s.length >= 2 && /^[a-z][a-z.]*$/.test(s)).length;
  return wordish >= 2;
}

function stripAnsi(line: string): string {
  return line.replace(ANSI, '');
}

/** Runs of whitespace (and the tab's column padding) become one space. */
function collapse(line: string): string {
  return line.replace(/\s+/g, ' ').trim();
}

/**
 * A line with no letter and no digit is box drawing, a progress bar, a spinner
 * or a rule — art that costs tokens and says nothing.
 */
function hasWords(line: string): boolean {
  return /[A-Za-z0-9]/.test(line);
}

/**
 * Mask every secret shape in one line, keeping the surrounding words so the
 * line still reads. Applied to every line before it can be sent anywhere.
 */
export function redactSecrets(line: string): string {
  // A private-key fence means the whole line is (or fences) key material.
  if (PRIVATE_KEY.test(line)) return REDACTED;

  let out = line;

  // Named assignments first: they are the only rule that keeps part of the
  // match, so running them before the shape rules keeps the name readable.
  out = out.replace(ASSIGNMENT, (_m, lead: string, name: string, sep: string) => `${lead}${name}${sep}${REDACTED}`);

  out = out.replace(AUTH_SCHEME, (_m, scheme: string) => `${scheme} ${REDACTED}`);
  out = out.replace(PREFIXED_TOKEN, REDACTED);

  out = out.replace(BARE_HEX, (_m, lead: string) => `${lead}${REDACTED}`);
  out = out.replace(BARE_BASE64, (match: string, lead: string, body: string) =>
    looksRandom(body) && !looksLikePath(body) ? `${lead}${REDACTED}` : match,
  );

  return out;
}

export interface ExcerptOptions {
  maxChars?: number;
  maxLines?: number;
}

/**
 * The compact excerpt a model sees for one tab: the tail of the screen, with
 * every secret masked, every escape sequence gone, art dropped and whitespace
 * collapsed, joined with ' / ' and hard-capped.
 *
 * The tail rather than the head because a terminal's newest output is at the
 * bottom, and the bottom is what the user means by "the tab where the build
 * failed".
 */
export function excerptFor(lines: string[], opts: ExcerptOptions = {}): string {
  const maxChars = opts.maxChars ?? 600;
  const maxLines = opts.maxLines ?? 15;
  if (maxChars <= 0 || maxLines <= 0) return '';

  // Blank lines are dropped before the tail is taken, so a screen padded out
  // with empty rows still contributes its last real lines.
  const nonBlank: string[] = [];
  for (const raw of lines) {
    const clean = collapse(stripAnsi(raw));
    if (clean !== '') nonBlank.push(clean);
  }

  const tail = nonBlank.slice(Math.max(0, nonBlank.length - maxLines));
  const parts: string[] = [];
  for (const line of tail) {
    const redacted = collapse(redactSecrets(line));
    // A line that is nothing but a mask carries no information and is not
    // worth the tokens; art and now-empty lines go the same way.
    if (redacted === '' || redacted === REDACTED || !hasWords(redacted)) continue;
    parts.push(redacted);
  }

  return truncateAtWord(parts.join(' / '), maxChars);
}

/** Cut at the last space before the limit, so no word is sliced in half. */
function truncateAtWord(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text;
  const cut = text.slice(0, maxChars - 1);
  const lastSpace = cut.lastIndexOf(' ');
  // Only honour the word boundary if it is not absurdly early; a single very
  // long token would otherwise truncate to almost nothing.
  const body = lastSpace > Math.floor(maxChars * 0.6) ? cut.slice(0, lastSpace) : cut;
  return `${body.trimEnd()}…`;
}

function queryWords(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter((w) => w.length > 0);
}

/**
 * The local, offline match, run before any model is involved: every word of
 * the query has to appear somewhere in the screen text. Word order does not
 * matter, case does not matter, and an empty query matches nothing (an empty
 * palette should not claim every tab).
 */
export function matchesScreen(query: string, lines: string[]): boolean {
  const words = queryWords(query);
  if (words.length === 0) return false;
  const haystack = lines
    .map((line) => collapse(stripAnsi(line)))
    .join(' ')
    .toLowerCase();
  if (haystack === '') return false;
  return words.every((word) => haystack.includes(word));
}

/**
 * The one line of evidence shown in the palette row's hint: the first line
 * that carries the match, windowed around it.
 *
 * Not redacted, deliberately — this is the screen the user is looking at, it
 * never leaves the process, and masking it would hide the very text they
 * typed. `excerptFor` is the redacting path because `excerptFor` is the one
 * that leaves.
 */
export function screenSnippet(query: string, lines: string[], width = 60): string | null {
  const words = queryWords(query);
  if (words.length === 0 || width <= 0) return null;

  const cleaned = lines.map((line) => collapse(stripAnsi(line))).filter((line) => line !== '');

  // Prefer a line that carries the whole query; fall back to one carrying the
  // first word, for a query whose words are spread over several lines.
  let best: { line: string; at: number } | null = null;
  for (const line of cleaned) {
    const lower = line.toLowerCase();
    const hits = words.map((word) => lower.indexOf(word));
    if (hits.every((at) => at >= 0)) {
      best = { line, at: Math.min(...hits) };
      break;
    }
    if (!best) {
      const first = lower.indexOf(words[0] ?? '');
      if (first >= 0) best = { line, at: first };
    }
  }
  if (!best) return null;

  return windowAround(best.line, best.at, words[0]?.length ?? 0, width);
}

/** Centre `width` characters on the match, with … where text was cut away. */
function windowAround(line: string, at: number, matchLength: number, width: number): string {
  if (line.length <= width) return line;

  const slack = Math.max(0, width - matchLength);
  let start = Math.max(0, at - Math.floor(slack / 2));
  start = Math.min(start, Math.max(0, line.length - width));
  const end = Math.min(line.length, start + width);

  const head = start > 0 ? '…' : '';
  const tailMark = end < line.length ? '…' : '';
  return `${head}${line.slice(start, end).trim()}${tailMark}`;
}
