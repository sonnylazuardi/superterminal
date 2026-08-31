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
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'space-between',
        gap: 12,
        paddingLeft: 12,
        paddingRight: 12,
        paddingTop: 8,
        paddingBottom: 8,
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
        }}
      >
        {content.text}
      </text>
      {content.action ? (
        <div
          testId="banner-action"
          onClick={content.action.run}
          style={{
            paddingLeft: 10,
            paddingRight: 10,
            paddingTop: 4,
            paddingBottom: 4,
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
      anchor="bottomRight"
      offset={{ x: -16, y: -16 }}
      style={{ flexDirection: 'column', gap: 6 }}
    >
      {toasts.map((toast) => (
        <div
          key={toast.id}
          testId={`toast-${toast.id}`}
          onClick={() => store.dispatch({ type: 'toast.dismiss', id: toast.id })}
          style={{
            paddingLeft: 12,
            paddingRight: 12,
            paddingTop: 6,
            paddingBottom: 6,
            borderRadius: tokens.radius.tab,
            backgroundColor: tokens.bg.overlay,
            borderWidth: tokens.border.width,
            borderColor: tokens.border.glass,
            cursor: 'pointer',
          }}
        >
          <text
            style={{
              color: toast.kind === 'error' ? tokens.fg.danger : tokens.fg.primary,
              fontSize: tokens.font.chip,
            }}
          >
            {toast.text}
          </text>
        </div>
      ))}
    </anchored>
  );
}
