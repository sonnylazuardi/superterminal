/**
 * Everything `app.tsx` does apart from calling `render()` (05 §1):
 *
 *   parse argv -> load config -> ensure server -> connect control plane
 *
 * Kept separate from the entry so it never imports `@gpuix/react` (and so the
 * native module is not required to exercise it).
 */

import { buildRegistry, type CommandRegistry } from './commands/registry.js';
import type { AppBridge, CommandContext } from './commands/types.js';
import { loadConfigAndWarn } from './config/load.js';
import type { Config } from './config/schema.js';
import { ControlClient } from './control/client.js';
import { detectPlatform, type PlatformInfo } from './platform/detect.js';
import { createCommandBus, createNativeBridge, type NativeCommandBus } from './native/bridge.js';
import { ensureServer, ServerUnavailableError } from './server/ensure.js';
import { defaultSocketPath } from './server/paths.js';
import { createWorkspaceStore, getOrCreateGlobalStore, type WorkspaceStore } from './state/workspace-store.js';
import { tokensFor, type Tokens } from './theme/tokens.js';
import { resolveBackground } from './platform/window-options.js';
import { debug } from './util/debug.js';
import type { Argv } from './cli/argv.js';

const log = debug('st:boot');

export interface Bootstrapped {
  config: Config;
  platform: PlatformInfo;
  tokens: Tokens;
  store: WorkspaceStore;
  registry: CommandRegistry;
  client: ControlClient;
  commandBus: NativeCommandBus;
  commandContext: CommandContext;
  socketPath: string;
}

export interface BootstrapOptions {
  argv: Argv;
  /** Dev: keep the store on globalThis so `bun --hot` preserves it. */
  useGlobalStore?: boolean;
  onQuit?: () => void;
}

export function bootstrap(options: BootstrapOptions): Bootstrapped {
  const { argv } = options;
  const platform = detectPlatform();
  const { config } = loadConfigAndWarn(argv.config ? { path: argv.config } : {});
  const background = resolveBackground(config, platform);
  log(`window background: ${background}`);
  const tokens = tokensFor(background);

  const store = options.useGlobalStore ? getOrCreateGlobalStore() : createWorkspaceStore();
  store.dispatch({ type: 'ui.setVerticalTabs', value: config.window.verticalTabs });

  const socketPath = argv.socket ?? defaultSocketPath();
  const client = new ControlClient({
    socketPath,
    reconnect: true,
    onRepeatedFailure: async () => {
      // The daemon may have exited idle; try to bring one back (05 §2).
      try {
        await ensureServer({ socketPath, noSpawn: argv.noSpawn });
      } catch (err) {
        log('ensureServer during reconnect failed', err);
      }
    },
  });

  wireClientToStore(client, store);

  const commandBus = createCommandBus();
  const registry = buildRegistry({ platform: platform.platform, overrides: config.keybindings });
  const app: AppBridge = {
    reconnect: async () => {
      store.dispatch({ type: 'connection.set', status: 'reconnecting' });
      try {
        await ensureServer({ socketPath, noSpawn: argv.noSpawn });
      } catch {
        /* the client's own retry loop reports the failure */
      }
      await connect(client, store, argv);
    },
    quit: () => {
      client.close();
      options.onQuit?.();
    },
  };

  const commandContext: CommandContext = {
    store,
    client,
    native: createNativeBridge({ bus: commandBus }),
    app,
    platform: platform.platform,
  };

  return {
    config,
    platform,
    tokens,
    store,
    registry,
    client,
    commandBus,
    commandContext,
    socketPath,
  };
}

/** Push every control-plane event and connection change into the store. */
export function wireClientToStore(client: ControlClient, store: WorkspaceStore): () => void {
  const offEvent = client.on((event) => store.applyEvent(event));
  const offState = client.onStateChange((state, error) => {
    switch (state) {
      case 'connected': {
        const info = client.serverInfo;
        store.dispatch({
          type: 'connection.set',
          status: 'connected',
          ...(info ? { serverVersion: info.protoVersion, serverBuildId: info.buildId } : {}),
        });
        return;
      }
      case 'connecting':
        store.dispatch({ type: 'connection.set', status: 'connecting' });
        return;
      case 'reconnecting':
        store.dispatch({
          type: 'connection.set',
          status: 'reconnecting',
          ...(error ? { error: error.message } : {}),
        });
        return;
      case 'closed':
      default:
        store.dispatch({
          type: 'connection.set',
          status: error?.name === 'VersionMismatchError' ? 'mismatch' : 'failed',
          ...(error ? { error: error.message } : {}),
        });
    }
  });
  return () => {
    offEvent();
    offState();
  };
}

/**
 * Connect and subscribe. Never throws: the window must appear even when the
 * server is broken — the user needs to see *why* (05 §1 step 4).
 */
export async function connect(
  client: ControlClient,
  store: WorkspaceStore,
  argv: Pick<Argv, 'noSpawn' | 'socket'>,
): Promise<void> {
  try {
    await ensureServer({ ...(argv.socket ? { socketPath: argv.socket } : {}), noSpawn: argv.noSpawn });
  } catch (err) {
    if (err instanceof ServerUnavailableError) {
      log(`ensureServer: ${err.kind}: ${err.message}`);
    } else {
      log('ensureServer threw', err);
    }
    // Fall through: the client tries anyway and reports the failure itself.
  }
  try {
    await client.connect();
    const snapshot = await client.request('workspace.subscribe', {});
    store.applySnapshot(snapshot);
  } catch (err) {
    const error = err as Error;
    store.dispatch({
      type: 'connection.set',
      status: error.name === 'VersionMismatchError' ? 'mismatch' : 'failed',
      error: error.message,
    });
  }
}
