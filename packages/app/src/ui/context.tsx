/**
 * The one React context the chrome needs: store, control client, command
 * registry, resolved tokens and platform. Components read it with the hooks
 * below instead of taking a dozen props.
 */

import { createContext, useContext, type ReactNode } from 'react';
import type { Config } from '../config/schema.js';
import type { CommandRegistry } from '../commands/registry.js';
import type { CommandContext } from '../commands/types.js';
import type { PlatformInfo } from '../platform/detect.js';
import { useWorkspaceSelector, useWorkspaceState } from '../state/use-workspace.js';
import type { WorkspaceState } from '../state/types.js';
import type { WorkspaceStore } from '../state/workspace-store.js';
import type { Tokens } from '../theme/tokens.js';
import type { NativeCommandBus } from '../native/bridge.js';

export interface AppServices {
  store: WorkspaceStore;
  registry: CommandRegistry;
  /** Everything a command needs; built once in `app.tsx`. */
  commandContext: CommandContext;
  tokens: Tokens;
  platform: PlatformInfo;
  config: Config;
  /** One-shot `<terminal-grid>` commands (copy/paste/clear). */
  commandBus: NativeCommandBus;
  /**
   * Socket the daemon listens on. Passed down to `<terminal-grid>`, which
   * opens its own data-plane connection to it (Q13/Q14).
   */
  socketPath: string;
}

const AppContext = createContext<AppServices | null>(null);

export function AppProvider(props: { services: AppServices; children: ReactNode }) {
  return <AppContext.Provider value={props.services}>{props.children}</AppContext.Provider>;
}

export function useServices(): AppServices {
  const services = useContext(AppContext);
  if (!services) throw new Error('useServices must be used inside <AppProvider>');
  return services;
}

export function useWorkspace<T>(selector: (state: WorkspaceState) => T): T {
  const { store } = useServices();
  return useWorkspaceSelector(store, selector);
}

export function useFullWorkspace(): WorkspaceState {
  const { store } = useServices();
  return useWorkspaceState(store);
}

/** Run a registry command with the ambient context. */
export function useRunCommand(): (id: string, arg?: number | string) => void {
  const { registry, commandContext } = useServices();
  return (id, arg) => {
    void Promise.resolve(registry.run(id, commandContext, arg)).catch((err: unknown) => {
      commandContext.store.dispatch({
        type: 'toast.push',
        text: err instanceof Error ? err.message : String(err),
        kind: 'error',
      });
    });
  };
}
