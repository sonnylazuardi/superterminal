/**
 * The palette's Jev round trip (08 Q12): one request per settled query,
 * debounced, single in-flight, stale answers discarded, answers cached per
 * query so backspacing is free. Pure timing/state; the merge itself lives in
 * `ai/palette-rank.ts`.
 */

import { useEffect, useRef, useState } from 'react';
import { buildRankingRequest, parseRanking, queryWorthAsking, type Candidate, type Ranking } from '../ai/palette-rank.js';
import type { AiService } from '../ai/service.js';
import type { WorkspaceStore } from '../state/workspace-store.js';
import { debug } from '../util/debug.js';

const log = debug('st:ai');

export const RANK_DEBOUNCE_MS = 250;

export interface RankingState {
  /** The ranking for `forQuery`, or null. */
  ranking: Ranking | null;
  forQuery: string;
  inFlight: boolean;
}

export function usePaletteRanking(input: {
  ai: AiService | null;
  store: WorkspaceStore;
  open: boolean;
  query: string;
  candidates: Candidate[];
  activeTabLabel: string | null;
}): RankingState {
  const { ai, store, open, query } = input;
  const [state, setState] = useState<RankingState>({ ranking: null, forQuery: '', inFlight: false });
  const cache = useRef(new Map<string, Ranking | null>());
  const abort = useRef<AbortController | null>(null);
  const seq = useRef(0);
  // The candidate list changes identity every render; keep the latest for the timer.
  const latest = useRef(input);
  latest.current = input;

  useEffect(() => {
    if (!open) {
      cache.current.clear();
      abort.current?.abort();
      abort.current = null;
      setState({ ranking: null, forQuery: '', inFlight: false });
      store.dispatch({ type: 'ai.setStatus', status: { busy: false } });
      return;
    }
    const trimmed = query.trim();
    if (!ai || !ai.available() || !queryWorthAsking(trimmed)) {
      abort.current?.abort();
      abort.current = null;
      setState({ ranking: null, forQuery: trimmed, inFlight: false });
      return;
    }
    const cached = cache.current.get(trimmed);
    if (cached !== undefined) {
      setState({ ranking: cached, forQuery: trimmed, inFlight: false });
      return;
    }

    const mySeq = ++seq.current;
    const timer = setTimeout(() => {
      if (mySeq !== seq.current) return;
      abort.current?.abort();
      const controller = new AbortController();
      abort.current = controller;
      const { candidates, activeTabLabel } = latest.current;
      const request = buildRankingRequest(trimmed, candidates, activeTabLabel);
      setState((s) => ({ ...s, inFlight: true }));
      store.dispatch({ type: 'ai.setStatus', status: { busy: true } });
      void ai
        .evaluate(request.state, request.questions, { signal: controller.signal })
        .then((result) => {
          const ranking = parseRanking(result.answers);
          cache.current.set(trimmed, ranking);
          if (log.enabled) {
            log(`query=${JSON.stringify(trimmed)} pick=${JSON.stringify(ranking?.picks.slice(0, 3))} any=${ranking?.anyMatch} ${result.latencyMs}ms`);
          }
          if (mySeq !== seq.current) return;
          setState({ ranking, forQuery: trimmed, inFlight: false });
        })
        .catch(() => {
          if (mySeq !== seq.current) return;
          setState({ ranking: null, forQuery: trimmed, inFlight: false });
        })
        .finally(() => {
          if (mySeq === seq.current) store.dispatch({ type: 'ai.setStatus', status: { busy: false } });
        });
    }, RANK_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [ai, open, query, store]);

  return state;
}
