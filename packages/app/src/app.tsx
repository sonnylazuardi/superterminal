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
 */

import { render, type Root } from '@gpuix/react';
import { bootstrap, connect } from './bootstrap.js';
import { parseArgv, USAGE } from './cli/argv.js';
import { buildWindowOptions } from './platform/window-options.js';
import { App } from './ui/App.js';
import type { AppServices } from './ui/context.js';

const BUILD_ID = process.env['SUPERTERMINAL_BUILD_ID'] ?? 'dev';
const VERSION = process.env['SUPERTERMINAL_VERSION'] ?? '0.1.0';

interface RootSlot {
  root: Root;
  services: AppServices;
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

  const windowOptions = buildWindowOptions(boot.config, boot.platform);
  const root = render(<App services={services} />, {
    ...windowOptions,
    onEvent: (event) => {
      if (event.eventType === 'windowResize') {
        const size = event as unknown as { width?: number; height?: number };
        boot.store.dispatch({
          type: 'window.resize',
          width: Number(size.width ?? 0),
          height: Number(size.height ?? 0),
        });
      }
    },
  });
  globalThis.__stRoot = { root, services };

  // Connecting is deliberately not awaited: the window must appear even when
  // the server is broken, with a banner explaining why (05 §1 step 4).
  void connect(boot.client, boot.store, argv);
}

if (import.meta.main) main();
