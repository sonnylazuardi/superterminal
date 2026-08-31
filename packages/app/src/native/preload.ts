/**
 * bunfig preload (`[run]` and `[test]`). Runs before any module graph is
 * evaluated, so `NAPI_RS_NATIVE_LIBRARY_PATH` is set before `@gpuix/react`
 * pulls in `@gpuix/native` (ESM imports hoist; doing this in `app.tsx` would be
 * too late — 05 §8).
 *
 * Never throws: `bun test` runs on machines with no built `.node`.
 */

import { locateNative, type LocateOutcome } from './locate.js';

let outcome: LocateOutcome | undefined;

export function preloadNative(): LocateOutcome {
  if (!outcome) outcome = locateNative();
  return outcome;
}

export function lastOutcome(): LocateOutcome | undefined {
  return outcome;
}

preloadNative();
