import { describe, expect, test } from 'bun:test';
import { buildRankingRequest, mergeRanking, parseRanking, queryWorthAsking, type Ranking } from './palette-rank.js';

const rows = ['cmd:tab.new', 'cmd:tab.close', 'tab:1', 'tab:2', 'session:1'].map((key) => ({ key }));
const byKey = new Map(rows.map((r) => [r.key, r] as const));
const ranking = (picks: Array<[string, number]>, anyMatch: number): Ranking => ({
  picks: picks.map(([id, probability]) => ({ id, probability })),
  anyMatch,
  confidence: 0.9,
});

describe('queryWorthAsking', () => {
  test('two non-space characters', () => {
    expect(queryWorthAsking('')).toBe(false);
    expect(queryWorthAsking(' a ')).toBe(false);
    expect(queryWorthAsking('ab')).toBe(true);
    expect(queryWorthAsking('a b')).toBe(true);
  });
});

describe('buildRankingRequest', () => {
  test('one choice over every candidate plus the any_match gate', () => {
    const req = buildRankingRequest('kill this tab', [{ id: 'cmd:tab.close', kind: 'command', label: 'Close Tab — …' }], 'Tab 1 · ~');
    expect(req.questions['pick']).toMatchObject({ type: 'choice', criteria: { 'cmd:tab.close': 'Close Tab — …' } });
    expect(req.questions['any_match']).toMatchObject({ type: 'noul' });
    expect(req.state).toMatchObject({ query: 'kill this tab', active_tab: 'Tab 1 · ~' });
  });
});

describe('parseRanking', () => {
  test('sorts the distribution and reads the gate', () => {
    const r = parseRanking({
      pick: { choice: 'a', probabilities: { a: 0.7, b: 0.3 }, confidence: 0.8 },
      any_match: { noul: 0.9 },
    });
    expect(r).toEqual({ picks: [{ id: 'a', probability: 0.7 }, { id: 'b', probability: 0.3 }], anyMatch: 0.9, confidence: 0.8 });
    expect(parseRanking({})).toBeNull();
  });
});

describe('mergeRanking (08 Q13)', () => {
  const local = [byKey.get('cmd:tab.new')!, byKey.get('tab:1')!];

  test('no ranking: local order untouched', () => {
    expect(mergeRanking({ local, byKey, ranking: null, selectedIndex: 0 })).toEqual({ rows: local, promoted: new Set() });
  });

  test('regime 1: a confident pick is promoted to row 0 and marked', () => {
    const out = mergeRanking({ local, byKey, ranking: ranking([['cmd:tab.close', 0.8]], 0.9), selectedIndex: 0 });
    expect(out.rows.map((r) => r.key)).toEqual(['cmd:tab.close', 'cmd:tab.new', 'tab:1']);
    expect(out.promoted).toEqual(new Set(['cmd:tab.close']));
  });

  test('regime 1: a pick already on top is only marked', () => {
    const out = mergeRanking({ local, byKey, ranking: ranking([['cmd:tab.new', 0.8]], 0.9), selectedIndex: 0 });
    expect(out.rows).toBe(local);
    expect(out.promoted).toEqual(new Set(['cmd:tab.new']));
  });

  test('regime 1: never while the user has moved the selection', () => {
    const out = mergeRanking({ local, byKey, ranking: ranking([['cmd:tab.close', 0.99]], 0.99), selectedIndex: 1 });
    expect(out.rows).toBe(local);
    expect(out.promoted.size).toBe(0);
  });

  test('regime 1: below the thresholds nothing moves', () => {
    expect(mergeRanking({ local, byKey, ranking: ranking([['cmd:tab.close', 0.5]], 0.9), selectedIndex: 0 }).rows).toBe(local);
    expect(mergeRanking({ local, byKey, ranking: ranking([['cmd:tab.close', 0.9]], 0.4), selectedIndex: 0 }).rows).toBe(local);
  });

  test('regime 2: an empty local list is filled in probability order, capped', () => {
    const picks: Array<[string, number]> = [['tab:2', 0.5], ['cmd:tab.close', 0.3], ['tab:1', 0.15], ['session:1', 0.05]];
    const out = mergeRanking({ local: [], byKey, ranking: ranking(picks, 0.8), selectedIndex: 0 });
    expect(out.rows.map((r) => r.key)).toEqual(['tab:2', 'cmd:tab.close', 'tab:1']);
    expect(out.promoted).toEqual(new Set(['tab:2', 'cmd:tab.close', 'tab:1']));
  });

  test('regime 2: stays empty when Jev doubts any candidate matches', () => {
    const out = mergeRanking({ local: [], byKey, ranking: ranking([['tab:2', 0.6]], 0.2), selectedIndex: 0 });
    expect(out.rows).toEqual([]);
  });
});
