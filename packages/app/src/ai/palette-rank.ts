/**
 * How Jev's answer merges with the local fuzzy list (08 Q13). Pure.
 *
 * Two regimes, decided by whether local matching found anything:
 *  1. Local has rows: Jev may PROMOTE its top pick to row 0, only when it is
 *     confident, the state agrees a candidate matches, and the user has not
 *     moved the selection. Everything else keeps fuzzy order.
 *  2. Local has nothing: the list is FILLED with Jev's candidates in
 *     probability order, when the state agrees a candidate matches.
 *
 * The thresholds are constants here; `scripts/jev-palette-probe.ts` is the
 * fixture run that justifies them.
 */

import type { JevAnswer, JevChoiceAnswer, JevQuestion } from './jev.js';

export interface Candidate {
  /** Stable row key, e.g. `cmd:tab.close`, `tab:7`, `session:2`. */
  id: string;
  kind: 'command' | 'tab' | 'session';
  /** What Jev reads: title plus the plain-words detail. */
  label: string;
}

export interface Ranking {
  /** Candidate ids in Jev's preferred order, with their probabilities. */
  picks: Array<{ id: string; probability: number }>;
  /** Jev's belief that at least one candidate satisfies the query. */
  anyMatch: number;
  confidence: number;
}

export const PROMOTE_MIN_PROBABILITY = 0.6;
export const PROMOTE_MIN_ANY_MATCH = 0.5;
export const FILL_MIN_ANY_MATCH = 0.5;
export const FILL_MIN_PROBABILITY = 0.1;
export const FILL_MAX_ROWS = 8;

/** Minimum query for a Jev call: two non-space characters (08 Q12). */
export function queryWorthAsking(query: string): boolean {
  return query.replace(/\s+/g, '').length >= 2;
}

/** The one request per query (08 Q11). */
export function buildRankingRequest(
  query: string,
  candidates: Candidate[],
  activeTab: string | null,
): { state: unknown; questions: Record<string, JevQuestion> } {
  const criteria: Record<string, string> = {};
  for (const c of candidates) criteria[c.id] = c.label;
  return {
    state: {
      query,
      active_tab: activeTab,
      candidates: candidates.map((c) => ({ id: c.id, kind: c.kind, label: c.label })),
    },
    questions: {
      pick: {
        type: 'choice',
        instructions:
          'Which candidate does the `query` ask for? The query is what a user typed into a ' +
          'terminal command palette. Prefer a tab when the query names a place, project, ' +
          'directory or describes a tab by its state (busy, exited, active); a command when it ' +
          'names an action; a session when it names a session.',
        criteria,
      },
      any_match: {
        type: 'noul',
        instructions:
          'Does at least one candidate genuinely satisfy the `query`? Low when the query is ' +
          'gibberish or asks for something no candidate offers.',
      },
    },
  };
}

export function parseRanking(answers: Record<string, JevAnswer>): Ranking | null {
  const pick = answers['pick'] as JevChoiceAnswer | undefined;
  const any = answers['any_match'] as { noul?: number } | undefined;
  if (!pick || typeof pick.choice !== 'string' || !pick.probabilities) return null;
  const picks = Object.entries(pick.probabilities)
    .map(([id, probability]) => ({ id, probability }))
    .sort((a, b) => b.probability - a.probability);
  return { picks, anyMatch: any?.noul ?? 0, confidence: pick.confidence ?? 0 };
}

export interface MergeInput<Row extends { key: string }> {
  /** Local fuzzy rows, best first. */
  local: Row[];
  /** Every candidate row the palette could show, by key. */
  byKey: Map<string, Row>;
  ranking: Ranking | null;
  /** `ui.paletteIndex`: a moved selection is never disturbed. */
  selectedIndex: number;
}

export interface MergeOutput<Row> {
  rows: Row[];
  /** Keys of rows Jev placed (for the ✦ marker). */
  promoted: Set<string>;
}

export function mergeRanking<Row extends { key: string }>(input: MergeInput<Row>): MergeOutput<Row> {
  const { local, byKey, ranking, selectedIndex } = input;
  const promoted = new Set<string>();
  if (!ranking) return { rows: local, promoted };

  if (local.length > 0) {
    const top = ranking.picks[0];
    if (
      top &&
      selectedIndex === 0 &&
      top.probability >= PROMOTE_MIN_PROBABILITY &&
      ranking.anyMatch >= PROMOTE_MIN_ANY_MATCH
    ) {
      const row = byKey.get(top.id);
      if (row && local[0]?.key !== top.id) {
        promoted.add(top.id);
        return { rows: [row, ...local.filter((r) => r.key !== top.id)], promoted };
      }
      if (row) promoted.add(top.id);
    }
    return { rows: local, promoted };
  }

  if (ranking.anyMatch < FILL_MIN_ANY_MATCH) return { rows: local, promoted };
  const rows: Row[] = [];
  for (const pick of ranking.picks) {
    if (pick.probability < FILL_MIN_PROBABILITY) break;
    const row = byKey.get(pick.id);
    if (!row) continue;
    rows.push(row);
    promoted.add(pick.id);
    if (rows.length >= FILL_MAX_ROWS) break;
  }
  return { rows, promoted };
}
