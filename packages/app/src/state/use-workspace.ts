/**
 * React bindings for the store. Kept apart from `workspace-store.ts` so the
 * reducers and the store stay importable (and testable) without React.
 */

import { useCallback, useDebugValue, useSyncExternalStore } from 'react';
import type { WorkspaceState } from './types.js';
import type { WorkspaceStore } from './workspace-store.js';

/**
 * Subscribe to a projection of the state. The selector must be stable or
 * memoised per state reference (see `selectors.ts`) — `useSyncExternalStore`
 * re-renders whenever the selected value changes identity.
 */
export function useWorkspaceSelector<T>(
  store: WorkspaceStore,
  selector: (state: WorkspaceState) => T,
): T {
  const getSnapshot = useCallback(() => selector(store.getState()), [store, selector]);
  const value = useSyncExternalStore(store.subscribe, getSnapshot, getSnapshot);
  useDebugValue(value);
  return value;
}

export function useWorkspaceState(store: WorkspaceStore): WorkspaceState {
  return useSyncExternalStore(store.subscribe, store.getState, store.getServerState);
}
