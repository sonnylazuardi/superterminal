/**
 * Connection banner (05 §4): one message at a time with one action.
 *
 *   disconnected/failed → Reconnect
 *   mismatch            → Restart server (states that running processes die, Q31)
 *   reconnecting        → spinner text only, no action
 */

import type { ConnectionState } from '../state/types.js';
import { useRunCommand, useServices, useWorkspace } from './context.js';

interface BannerContent {
  text: string;
  action: { label: string; run: () => void } | null;
  danger: boolean;
}

export function bannerFor(
  connection: ConnectionState,
  actions: { reconnect: () => void; restartServer: () => void },
): BannerContent | null {
  switch (connection.status) {
    case 'connected':
      return null;
    case 'connecting':
      return { text: 'Connecting to superterminald…', action: null, danger: false };
    case 'reconnecting':
      return { text: 'Reconnecting…', action: null, danger: false };
    case 'mismatch':
      return {
        text:
          connection.error ??
          'This client and the running server speak different protocol versions.',
        action: {
          label: 'Restart server (kills running processes)',
          run: actions.restartServer,
        },
        danger: true,
      };
    case 'failed':
    case 'closed':
    default:
      return {
        text: connection.error ?? 'Disconnected from superterminald.',
        action: { label: 'Reconnect', run: actions.reconnect },
        danger: true,
      };
  }
}

export function Banner() {
  const { tokens, commandContext, store } = useServices();
  const connection = useWorkspace((s) => s.connection);
  const run = useRunCommand();

  const content = bannerFor(connection, {
    reconnect: () => run('app.reconnect'),
    restartServer: () => {
      void commandContext.client
        .request('server.shutdown', { force: true })
        .catch(() => {
          /* the server is going away; the reconnect loop takes over */
        })
        .finally(() => {
          store.dispatch({ type: 'connection.set', status: 'reconnecting' });
          run('app.reconnect');
        });
    },
  });
  if (!content) return null;

  return (
    <div
      testId="banner"
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'space-between',
        gap: tokens.space.lg,
        paddingLeft: tokens.space.xl,
        paddingRight: tokens.space.xl,
        paddingTop: tokens.space.lg,
        paddingBottom: tokens.space.lg,
        backgroundColor: tokens.bg.glass,
        borderBottomWidth: tokens.border.width,
        borderColor: tokens.border.glass,
      }}
    >
      <text
        testId="banner-text"
        style={{
          color: content.danger ? tokens.fg.danger : tokens.fg.muted,
          fontSize: tokens.font.chrome,
          flexGrow: 1,
          minWidth: 0,
          overflow: 'hidden',
        }}
      >
        {content.text}
      </text>
      {content.action ? (
        <div
          testId="banner-action"
          onClick={content.action.run}
          style={{
            display: 'flex',
            flexShrink: 0,
            paddingLeft: tokens.space.md,
            paddingRight: tokens.space.md,
            paddingTop: tokens.space.xs,
            paddingBottom: tokens.space.xs,
            borderRadius: tokens.radius.tab,
            backgroundColor: tokens.bg.glassActive,
            cursor: 'pointer',
            hover: { backgroundColor: tokens.bg.glassHover },
          }}
        >
          <text style={{ color: tokens.fg.primary, fontSize: tokens.font.chip }}>
            {content.action.label}
          </text>
        </div>
      ) : null}
    </div>
  );
}

export function StatusToasts() {
  const { tokens, store } = useServices();
  const toasts = useWorkspace((s) => s.ui.toasts);
  if (toasts.length === 0) return null;

  return (
    <anchored
      testId="toasts"
      side="bottom"
      align="end"
      anchor="bottomRight"
      offset={{ x: -tokens.space.xl, y: -tokens.space.xl }}
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: tokens.space.xs,
        // The panel surface lives on the STACK: an <anchored> layer whose
        // background resolves to alpha 0 is forced opaque #1A1A1A, which
        // would show as grey in the gaps between per-toast panels.
        backgroundColor: tokens.bg.overlay,
        borderRadius: tokens.radius.tab,
        borderWidth: tokens.border.width,
        borderColor: tokens.border.glass,
        padding: tokens.space.sm,
        maxWidth: tokens.strip.toastMaxWidth,
      }}
    >
      {toasts.map((toast) => (
        <div
          key={toast.id}
          testId={`toast-${toast.id}`}
          onClick={() => store.dispatch({ type: 'toast.dismiss', id: toast.id })}
          style={{
            display: 'flex',
            cursor: 'pointer',
          }}
        >
          <text
            style={{
              color: toast.kind === 'error' ? tokens.fg.danger : tokens.fg.primary,
              fontSize: tokens.font.chip,
              // No nowrap: long server errors wrap inside toastMaxWidth
              // instead of spanning the window.
              minWidth: 0,
            }}
          >
            {toast.text}
          </text>
        </div>
      ))}
    </anchored>
  );
}
