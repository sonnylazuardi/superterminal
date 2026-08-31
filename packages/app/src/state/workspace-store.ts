/**
 * The workspace store (05 §3): a ~60-line hand-rolled store shaped for
 * `useSyncExternalStore`. All logic lives in the pure reducers next door; this
 * file only holds the current state and notifies subscribers.
 */

import type { WorkspaceSnapshot } from '@superterminal/protocol-ts';
import { applyServerEvent, applyUiAction, initialWorkspaceState, snapshotEvent } from './reducers.js';
import type { ServerEvent, UiAction, WorkspaceState } from './types.js';

export interface WorkspaceStore {
  /** Stable identity while nothing changes — safe for `useSyncExternalStore`. */
  getState(): WorkspaceState;
  /** Same value on the server; there is no SSR, but React asks for it. */
  getServerState(): WorkspaceState;
  subscribe(listener: () => void): () => void;
  /** Fold a control-plane event (or a snapshot) into the projection. */
  applyEvent(event: ServerEvent): void;
  applySnapshot(snapshot: WorkspaceSnapshot): void;
  /** Client-only UI transition. */
  dispatch(action: UiAction): void;
  /** Test/dev helper: force a state. */
  replaceState(state: WorkspaceState): void;
  readonly listenerCount: number;
}

export function createWorkspaceStore(
  initial: WorkspaceState = initialWorkspaceState,
): WorkspaceStore {
  let state = initial;
  const listeners = new Set<() => void>();

  const set = (next: WorkspaceState): void => {
    if (next === state) return; // reducers return the same object for no-ops
    state = next;
    for (const listener of [...listeners]) listener();
  };

  return {
    getState: () => state,
    getServerState: () => state,
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    applyEvent(event) {
      set(applyServerEvent(state, event));
    },
    applySnapshot(snapshot) {
      set(applyServerEvent(state, snapshotEvent(snapshot)));
    },
    dispatch(action) {
      set(applyUiAction(state, action));
    },
    replaceState(next) {
      set(next);
    },
    get listenerCount() {
      return listeners.size;
    },
  };
}

/**
 * In dev the store lives on `globalThis` so `bun --hot` keeps state across a
 * module re-evaluation (05 §8).
 */
const GLOBAL_KEY = '__stWorkspaceStore';

export function getOrCreateGlobalStore(): WorkspaceStore {
  const existing = (globalThis as Record<string, unknown>)[GLOBAL_KEY];
  if (existing) return existing as WorkspaceStore;
  const store = createWorkspaceStore();
  (globalThis as Record<string, unknown>)[GLOBAL_KEY] = store;
  return store;
}

export { applyServerEvent, applyUiAction, initialWorkspaceState, snapshotEvent };
export type { ServerEvent, UiAction, WorkspaceState };
