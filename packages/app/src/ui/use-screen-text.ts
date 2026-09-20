/**
 * What each Tab is *showing*, for the palette (08 §H, "screen-content tab
 * search"). Titles and cwds answer "which tab is the api one"; they cannot
 * answer "the tab about the baby tracker" or "the one with the memory
 * monitor". The visible rows of every Pane can, so the palette asks for them
 * once when it opens.
 *
 * Once per opening, not per keystroke: the request costs a round trip, a
 * screen does not meaningfully change while a dialog is up, and the map is
 * dropped on close so nothing is held. It never blocks — the palette paints
 * from titles immediately and the rows re-rank when the map lands — and it
 * never fails loudly: a daemon that does not know the request (anything older
 * than this feature) yields an empty map, which is exactly today's palette.
 */

import { useEffect, useRef, useState } from 'react';
import { selectActiveTabs } from '../state/selectors.js';
import type { WorkspaceState } from '../state/types.js';
import { debug } from '../util/debug.js';

const log = debug('st:ai');

/** Short: the palette is already open and must not wait on a slow daemon. */
export const SCREEN_TEXT_TIMEOUT_MS = 1500;

/** The one request this hook makes; a `ControlClientLike` satisfies it. */
export interface ScreenTextClient {
  request(
    type: 'surface.screen_text',
    params: { surfaces: number[]; max_rows?: number },
    opts?: { timeoutMs?: number },
  ): Promise<{ screens: Array<{ surface: number; lines: string[] }> }>;
}

export interface ScreenTextState {
  /** Visible lines per Surface id; empty until the answer lands. */
  screens: Map<number, string[]>;
  /** The request settled — with rows, without, or in failure. */
  loaded: boolean;
}

/** One shared value, so a `setState` on a closed palette bails out early. */
const EMPTY: ScreenTextState = { screens: new Map(), loaded: false };

/** Every Pane's Surface of every Tab in the active Session, in tab order. */
export function screenTextSurfaceIds(state: WorkspaceState): number[] {
  const ids: number[] = [];
  for (const tab of selectActiveTabs(state)) {
    for (const id of tab.surfaceIds) if (!ids.includes(id)) ids.push(id);
  }
  return ids;
}

/**
 * One request for every Surface. Never rejects: a failure is the old-daemon
 * case and must degrade to "no screen text", not to a broken palette.
 */
export async function fetchScreenText(
  client: ScreenTextClient,
  surfaces: number[],
): Promise<Map<number, string[]>> {
  const screens = new Map<number, string[]>();
  if (surfaces.length === 0) return screens;
  try {
    const result = await client.request(
      'surface.screen_text',
      { surfaces },
      { timeoutMs: SCREEN_TEXT_TIMEOUT_MS },
    );
    // Unknown ids are omitted by the daemon, so this is a partial map by design.
    for (const screen of result.screens) screens.set(screen.surface, screen.lines);
  } catch (err) {
    log('surface.screen_text failed', err);
  }
  return screens;
}

export function useScreenText(input: {
  client: ScreenTextClient | null;
  open: boolean;
  state: WorkspaceState;
}): ScreenTextState {
  const { client, open } = input;
  const [screenText, setScreenText] = useState<ScreenTextState>(EMPTY);
  // The state object changes identity every render; the effect wants the
  // latest one at the moment the palette opens, not a dependency on it.
  const latest = useRef(input);
  latest.current = input;

  useEffect(() => {
    if (!open || !client) {
      setScreenText(EMPTY);
      return;
    }
    let live = true;
    void fetchScreenText(client, screenTextSurfaceIds(latest.current.state)).then((screens) => {
      if (live) setScreenText({ screens, loaded: true });
    });
    return () => {
      live = false;
    };
  }, [client, open]);

  return screenText;
}
