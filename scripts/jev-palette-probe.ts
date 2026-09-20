#!/usr/bin/env bun
/**
 * Live fixture run for the palette ranking thresholds (08 Q22): builds the
 * same candidates the palette would from `ai/fixtures/palette-queries.json`,
 * sends every query to Jev, and prints pick, probability, any_match and
 * latency. Needs a key: `[ai] api_key`, secrets.json, $SUPERTERMINAL_AI_API_KEY
 * or an OpenCode login. Run by hand before freezing the thresholds.
 *
 *   bun scripts/jev-palette-probe.ts
 */

import { createJevClient } from '../packages/app/src/ai/jev.js';
import { resolveApiKey } from '../packages/app/src/ai/key-source.js';
import { realSecretsFs, secretsPath } from '../packages/app/src/ai/key-store.js';
import {
  buildRankingRequest,
  FILL_MIN_ANY_MATCH,
  parseRanking,
  PROMOTE_MIN_ANY_MATCH,
  PROMOTE_MIN_PROBABILITY,
  type Candidate,
} from '../packages/app/src/ai/palette-rank.js';
import { buildRegistry } from '../packages/app/src/commands/registry.js';
import { loadConfig } from '../packages/app/src/config/load.js';
import fixtures from '../packages/app/src/ai/fixtures/palette-queries.json';

interface Tab { id: number; title: string; cwd: string; busy?: boolean; exited?: boolean; active?: boolean }
interface Session { id: number; name: string; tabs: Tab[] }

const { config } = loadConfig();
const { resolved } = resolveApiKey({ configKey: config.ai.apiKey, secretsPath: secretsPath(), fs: realSecretsFs });
if (!resolved) {
  console.error('no Jev key found (config, secrets.json, env or OpenCode login)');
  process.exit(2);
}
console.log(`key from ${resolved.source} · ${config.ai.model} at ${config.ai.endpoint}\n`);
const client = createJevClient({ endpoint: config.ai.endpoint, model: config.ai.model, getKey: () => resolved.key, timeoutMs: 10_000 });

const registry = buildRegistry({ platform: 'linux' });
const candidates: Candidate[] = [];
for (const c of registry.commands) {
  if (c.hidden) continue;
  candidates.push({ id: `cmd:${c.id}`, kind: 'command', label: `${c.title} — ${c.description}` });
}
let activeTab: string | null = null;
for (const session of fixtures.workspace.sessions as Session[]) {
  session.tabs.forEach((tab, i) => {
    const title = tab.title.replace(/^[^@\s:]+@[^\s:]+:/, '');
    const parts = [`Tab ${i + 1}`, title, tab.cwd.replace('/home/sonny', '~'), `session ${session.name}`];
    if (tab.exited) parts.push('exited');
    else if (tab.busy) parts.push('busy');
    if (tab.active) parts.push('active');
    const label = parts.join(' · ');
    if (tab.active) activeTab = label;
    candidates.push({ id: `tab:${tab.id}`, kind: 'tab', label });
  });
  candidates.push({
    id: `session:${session.id}`,
    kind: 'session',
    label: `Session ${session.name} · ${session.tabs.length} tab${session.tabs.length === 1 ? '' : 's'}${session.id === 1 ? ' · active' : ''}`,
  });
}

let pass = 0;
const latencies: number[] = [];
const rows: string[][] = [['query', 'expect', 'pick', 'p', 'any', 'ms', 'verdict']];
for (const c of fixtures.cases as Array<{ query: string; expect: string | null }>) {
  const req = buildRankingRequest(c.query, candidates, activeTab);
  const result = await client.evaluate(req.state, req.questions);
  const ranking = parseRanking(result.answers);
  const top = ranking?.picks[0];
  latencies.push(result.latencyMs);
  let ok: boolean;
  if (c.expect === null) {
    ok = (ranking?.anyMatch ?? 0) < FILL_MIN_ANY_MATCH;
  } else {
    ok =
      top?.id === c.expect &&
      top.probability >= PROMOTE_MIN_PROBABILITY &&
      (ranking?.anyMatch ?? 0) >= PROMOTE_MIN_ANY_MATCH;
  }
  if (ok) pass++;
  rows.push([
    c.query,
    c.expect ?? '(none)',
    top?.id ?? '-',
    top ? top.probability.toFixed(2) : '-',
    (ranking?.anyMatch ?? 0).toFixed(2),
    String(result.latencyMs),
    ok ? 'ok' : 'MISS',
  ]);
}
const widths = rows[0]!.map((_, i) => Math.max(...rows.map((r) => r[i]!.length)));
for (const r of rows) console.log(r.map((cell, i) => cell.padEnd(widths[i]!)).join('  '));
latencies.sort((a, b) => a - b);
const p = (q: number) => latencies[Math.min(latencies.length - 1, Math.floor(q * latencies.length))];
console.log(`\n${pass}/${fixtures.cases.length} pass · latency p50 ${p(0.5)} ms · p95 ${p(0.95)} ms · thresholds promote≥${PROMOTE_MIN_PROBABILITY}/any≥${PROMOTE_MIN_ANY_MATCH}`);
process.exit(pass === fixtures.cases.length ? 0 : 1);
