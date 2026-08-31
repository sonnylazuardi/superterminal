/**
 * Tiny `DEBUG=st:*` logger (05 §8). No dependency; writes to stderr.
 *
 *   const log = debug('st:control');
 *   log('connected to %s', path);
 */

export type Debugger = ((...args: unknown[]) => void) & { enabled: boolean; namespace: string };

function patternsFrom(spec: string | undefined): string[] {
  if (!spec) return [];
  return spec
    .split(/[\s,]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

export function namespaceEnabled(namespace: string, spec: string | undefined): boolean {
  let enabled = false;
  for (const raw of patternsFrom(spec)) {
    const negated = raw.startsWith('-');
    const pattern = negated ? raw.slice(1) : raw;
    const re = new RegExp(`^${pattern.split('*').map(escapeRe).join('.*')}$`);
    if (re.test(namespace)) enabled = !negated;
  }
  return enabled;
}

function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export function debug(namespace: string, spec = process.env['DEBUG']): Debugger {
  const enabled = namespaceEnabled(namespace, spec);
  const fn = ((...args: unknown[]) => {
    if (!fn.enabled) return;
    const parts = args.map((a) => (typeof a === 'string' ? a : inspect(a)));
    process.stderr.write(`${namespace} ${parts.join(' ')}\n`);
  }) as Debugger;
  fn.enabled = enabled;
  fn.namespace = namespace;
  return fn;
}

function inspect(v: unknown): string {
  if (v instanceof Error) return `${v.name}: ${v.message}`;
  try {
    return JSON.stringify(v) ?? String(v);
  } catch {
    return String(v);
  }
}
