/**
 * Entry point (05 §1, §8).
 *
 *   parse argv ──▶ load config ──▶ ensure server ──▶ connect control plane
 *      │
 *      └──▶ pick window options ──▶ render(<App/>) ──▶ store gets the snapshot
 *
 * `bun --hot` re-evaluates this module; `globalThis.__stRoot` makes sure the
 * gpuix window and the frame loop are created exactly once and later runs only
 * re-render into the existing root.
 *
 * The native module is located by `native/preload.ts`, which `bunfig.toml`
 * preloads — ESM imports hoist, so setting the env var here would be too late.
 * The static import below is the same guarantee for `bun build --compile`
 * binaries, which do not read bunfig preloads: it evaluates before
 * `@gpuix/react` (first import) pulls in `@gpuix/native`.
 */

import './native/preload.js';
// B.2.1: `DEBUG=st:mem` samples process.memoryUsage() + replicated bytes every
// 30 s. Must come after the preload so the first sample sees the native path.
import './debug/mem-log.js';
import { render, type Root } from '@gpuix/react';
import { bootstrap, connect } from './bootstrap.js';
import { parseArgv, USAGE } from './cli/argv.js';
import { buildWindowOptions } from './platform/window-options.js';
import {
  createClientStatePersister,
  type ClientState,
  type ClientStatePersister,
} from './state/client-state.js';
import { readWindowPlacement, type WindowPlacementSource } from './state/window-placement.js';
import type { WorkspaceState } from './state/types.js';
import type { WorkspaceStore } from './state/workspace-store.js';
import { App } from './ui/App.js';
import type { AppServices } from './ui/context.js';
import { debug } from './util/debug.js';
import { startDriveFile } from './util/drive.js';

const eventsLog = debug('st:events');

const BUILD_ID = process.env['SUPERTERMINAL_BUILD_ID'] ?? 'dev';
const VERSION = process.env['SUPERTERMINAL_VERSION'] ?? '0.1.0';

interface RootSlot {
  root: Root;
  services: AppServices;
  persister: ClientStatePersister | null;
}

declare global {
  // eslint-disable-next-line no-var
  var __stRoot: RootSlot | undefined;
}

export function main(argvInput: string[] = Bun.argv.slice(2)): void {
  const argv = parseArgv(argvInput);

  if (argv.help) {
    process.stdout.write(USAGE);
    return;
  }
  if (argv.version) {
    process.stdout.write(`superterminal ${VERSION} (${BUILD_ID}, proto 1.0)\n`);
    return;
  }

  // Hot reload: keep the window, the store and the socket; swap the tree.
  const existing = globalThis.__stRoot;
  if (existing) {
    existing.root.render(<App services={existing.services} />);
    return;
  }

  const boot = bootstrap({
    argv,
    useGlobalStore: true,
    onQuit: () => process.exit(0),
  });

  const services: AppServices = {
    store: boot.store,
    registry: boot.registry,
    commandContext: boot.commandContext,
    tokens: boot.tokens,
    platform: boot.platform,
    config: boot.config,
    commandBus: boot.commandBus,
    socketPath: boot.socketPath,
  };

  const windowOptions = buildWindowOptions(boot.config, boot.platform, boot.clientState.window);
  // The window size is sampled by <WindowSizeTracker/> inside the tree:
  // gpuix emits no resize event.
  const root = render(<App services={services} />, {
    ...windowOptions,
    // `DEBUG=st:events` traces every native event before React sees it:
    // the way to tell "gpuix never sent it" from "no handler matched".
    onEvent: (event) => {
      if (eventsLog.enabled) {
        const e = event as unknown as Record<string, unknown>;
        eventsLog(e['eventType'], 'el', e['elementId'], 'btn', e['button'], 'right', e['isRightClick'], 'key', e['key']);
      }
    },
  });
  const persister = boot.clientStatePath
    ? persistClientState(boot.store, boot.clientStatePath, boot.clientState)
    : null;
  globalThis.__stRoot = { root, services, persister };

  // The renderer lives in gpuix's render slot on globalThis; both the
  // placement sampler and the dev driver read it from there.
  const renderHost = (globalThis as Record<string, unknown>)['__gpuixRenderHost'] as
    | { renderer?: WindowPlacementSource & Parameters<typeof startDriveFile>[1] }
    | undefined;
  trackWindowPlacement(boot.store, renderHost?.renderer ?? null);

  // Dev-only scripted input (util/drive.ts).
  const drivePath = process.env['SUPERTERMINAL_DRIVE'];
  if (drivePath && renderHost?.renderer) startDriveFile(drivePath, renderHost.renderer);

  // Connecting is deliberately not awaited: the window must appear even when
  // the server is broken, with a banner explaining why (05 §1 step 4).
  void connect(boot.client, boot.store, argv);
}

/** What the store knows that is worth remembering (ADR 0008). */
export function clientStateOf(state: WorkspaceState): ClientState {
  const window = state.ui.window;
  return {
    // A zero size is "not measured yet"; the caller keeps the placement that
    // was loaded from disk instead of erasing it (see `persistClientState`).
    window: window.width > 0 && window.height > 0 ? window : null,
    verticalTabs: state.ui.verticalTabs,
    sidebarWidth: state.ui.sidebarWidth,
    fontZoom: state.ui.fontZoom,
  };
}

/** The placement is re-read at this rate when the window neither resizes nor moves. */
export const WINDOW_PLACEMENT_SAMPLE_MS = 2_000;

/**
 * Sample the Window Placement (CONTEXT.md, ADR 0008): after every resize
 * action and every 2 s. `getWindowPlacement` is a round trip to the UI thread,
 * so it is not polled per frame, and `ui/window-placed` returns the previous
 * state when nothing changed.
 */
export function trackWindowPlacement(
  store: WorkspaceStore,
  renderer: WindowPlacementSource | null,
): void {
  const sample = (): void => {
    const placement = readWindowPlacement(renderer);
    if (placement) store.dispatch({ type: 'ui/window-placed', placement });
  };
  let lastWindow = store.getState().ui.window;
  store.subscribe(() => {
    const next = store.getState().ui.window;
    // The interval catches moves; only a changed viewport size warrants an
    // immediate read, because that is what a resize action writes.
    if (next.width === lastWindow.width && next.height === lastWindow.height) return;
    lastWindow = next;
    sample();
  });
  sample();
  setInterval(sample, WINDOW_PLACEMENT_SAMPLE_MS);
}

/**
 * Write Client State as it changes, and once more on exit. The exit hook is
 * the path that always runs: closing the window from the title bar ends in
 * gpuix's `onTerminated → process.exit(0)`, not in the Quit command.
 */
function persistClientState(
  store: WorkspaceStore,
  path: string,
  initial: ClientState,
): ClientStatePersister {
  const persister = createClientStatePersister({
    path,
    initial,
    onError: (err) => {
      process.stderr.write(`[superterminal] could not write ${path}: ${String(err)}\n`);
    },
  });
  const push = () => {
    const next = clientStateOf(store.getState());
    // A window that has not been measured yet must not erase the last size.
    persister.push(next.window ? next : { ...next, window: initial.window });
  };
  store.subscribe(push);
  process.on('exit', () => persister.flush());
  return persister;
}

if (import.meta.main) main();
