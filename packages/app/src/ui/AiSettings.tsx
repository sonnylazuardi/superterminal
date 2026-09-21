/**
 * The AI Settings dialog (`ai.settings`, 08 Q17): where the Jev provider is
 * picked (Auto, TypeSafe or OpenCode Zen), where its key is entered and
 * stored, and where the state of the Jev integration is shown.
 *
 * The provider picked here is remembered as Client State and wins over
 * `[ai] provider` in config.toml, which the program never writes (ADR 0008).
 *
 * A Dialog like About — same placement, same panel. The key `<input>` takes
 * ⌘V / Ctrl+V itself (gpuix binds Paste on every desktop build), so the app's
 * `edit.paste` never sees the chord. There is no masked mode: the key is
 * visible while typed, the field is emptied on save, and afterwards only the
 * last four characters are shown. Enter saves and runs a connection test.
 */

import { useState } from 'react';
import { last4 as keyTail, validateKeyShape } from '../ai/key-store.js';
import { PROVIDERS, providerLabel } from '../ai/providers.js';
import type { AiStatus, ProviderId, ProviderSetting } from '../state/types.js';
import { dialogPlacement } from './CommandPalette.js';
import { useServices, useWorkspace } from './context.js';
import { ICONS, Icon } from './Icon.js';

const WIDTH = 520;

/** The row's click order. */
const PROVIDER_CYCLE: readonly ProviderSetting[] = ['auto', 'typesafe', 'zen'];

/** Auto → TypeSafe → OpenCode Zen → Auto. */
export function nextProviderSetting(setting: ProviderSetting): ProviderSetting {
  return PROVIDER_CYCLE[(PROVIDER_CYCLE.indexOf(setting) + 1) % PROVIDER_CYCLE.length]!;
}

/** "Auto · using TypeSafe", "Auto · no key found", "TypeSafe", "OpenCode Zen". */
export function providerSettingLabel(setting: ProviderSetting, provider: ProviderId | null): string {
  if (setting !== 'auto') return PROVIDERS[setting].label;
  return provider ? `Auto · using ${PROVIDERS[provider].label}` : 'Auto · no key found';
}

/** The key field names the provider the pasted key will be stored for. */
export function keyPlaceholder(setting: ProviderSetting): string {
  switch (setting) {
    case 'typesafe':
      return 'Paste a TypeSafe API key (console.typesafe.ai/keys)';
    case 'zen':
      return 'Paste an OpenCode Zen API key';
    default:
      return 'Paste a TypeSafe or OpenCode Zen key — apikey_… is TypeSafe';
  }
}

/** The provider named in the status line: the one in use, else the one asked for. */
function namedProvider(ai: AiStatus): ProviderId | null {
  return ai.provider ?? (ai.providerSetting === 'auto' ? null : ai.providerSetting);
}

/** The status line, pure for the tests. */
export function aiStatusLine(ai: AiStatus): string {
  if (!ai.enabled) return 'AI ranking is turned off in config.toml ([ai] palette = false)';
  const named = namedProvider(ai);
  switch (ai.status) {
    case 'off':
      return named ? `AI ranking: off · no ${PROVIDERS[named].label} key found` : 'AI ranking: off · no key found';
    case 'disabled':
      return `AI ranking: disabled this session · ${named ? `${PROVIDERS[named].label} · ` : ''}${ai.lastError ?? 'provider error'}`;
    case 'ready': {
      const source = { config: 'config.toml', app: 'this app', env: 'environment', opencode: 'OpenCode login', none: '' }[ai.source];
      const tail = ai.last4 ? ` · key …${ai.last4}` : '';
      const latency = ai.lastLatencyMs !== null ? ` · last call ${ai.lastLatencyMs} ms` : '';
      return `AI ranking: on · ${providerLabel(ai.provider)} · key from ${source}${tail}${latency}`;
    }
    default:
      return '';
  }
}

/** What leaves the machine while the palette is open. Pure, and honest. */
export function privacyLine(ai: AiStatus): string {
  const named = namedProvider(ai);
  const to = named ? PROVIDERS[named].label : 'the provider';
  const base = `Typing in the palette sends the query, command titles, tab titles, working directories and session names to ${to}.`;
  return ai.screenContext
    ? `${base} It also sends the last few visible lines of each tab; keys, tokens and passwords are masked first, but masking is a safety net, not a guarantee.`
    : `${base} Never screen contents.`;
}

export function AiSettings() {
  const { tokens, store, ai: service } = useServices();
  const open = useWorkspace((s) => s.ui.aiSettingsOpen);
  const ai = useWorkspace((s) => s.ui.ai);
  const windowWidth = useWorkspace((s) => s.ui.window.width);
  const vertical = useWorkspace((s) => s.ui.verticalTabs);
  const placement = dialogPlacement(windowWidth, tokens, vertical);
  const [draft, setDraft] = useState('');
  const [note, setNote] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);

  if (!open) return null;

  const close = () => {
    setDraft('');
    setNote(null);
    store.dispatch({ type: 'aiSettings.close' });
  };

  const save = () => {
    if (!service) return;
    const problem = validateKeyShape(draft);
    if (problem) {
      setNote(problem);
      return;
    }
    const key = draft.trim();
    // Under Auto the service routes the key by its shape (apikey_… is TypeSafe).
    const explicit = ai.providerSetting === 'auto' ? undefined : ai.providerSetting;
    try {
      service.setKey(key, explicit);
    } catch (err) {
      setNote(`Could not save the key: ${err instanceof Error ? err.message : String(err)}`);
      return;
    }
    setDraft('');
    setNote(`Saved key …${keyTail(key)} · testing…`);
    void runTest();
  };

  const runTest = async () => {
    if (!service) return;
    setTesting(true);
    const result = await service.testConnection();
    setTesting(false);
    // Read after the call: saving a key may have switched the provider in use.
    const now = service.snapshot();
    const label = providerLabel(now.provider);
    setNote(
      result.ok
        ? `Connection OK · ${label} · ${result.model ?? now.model} · ${result.latencyMs} ms`
        : `Connection failed · ${label} · ${result.error}`,
    );
  };

  // Remembered as Client State by the app's persister (app.tsx), never
  // written to config.toml.
  const cycleProvider = () => {
    if (!service) return;
    const next = nextProviderSetting(ai.providerSetting);
    service.setProvider(next);
    const now = service.snapshot();
    setNote(`Provider: ${providerSettingLabel(next, now.provider)}`);
  };

  // Session-only: the program never writes config.toml (project invariant),
  // so this lasts until the app quits unless `[ai] screen_context = true`.
  const toggleScreenContext = () => {
    store.dispatch({ type: 'ai.setStatus', status: { screenContext: !ai.screenContext } });
    setNote(
      ai.screenContext
        ? 'Screen text will no longer be sent'
        : 'Screen text will be sent with palette queries this session',
    );
  };

  const remove = () => {
    if (!service) return;
    service.removeKey(ai.provider ?? undefined);
    setNote(`Removed the app-stored ${providerLabel(ai.provider)} key`);
  };

  const row = (testId: string, label: string, hint: string, onClick: () => void, accent = false) => (
    <div
      testId={testId}
      onClick={onClick}
      style={{
        userSelect: 'none',
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        height: tokens.strip.rowHeight,
        paddingLeft: tokens.space.lg,
        paddingRight: tokens.space.lg,
        gap: tokens.space.lg,
        borderRadius: tokens.radius.tab,
        backgroundColor: accent ? tokens.bg.glassActive : 'transparent',
        borderWidth: tokens.border.width,
        borderColor: accent ? tokens.accent : 'transparent',
        cursor: 'pointer',
        hover: { backgroundColor: tokens.bg.glassHover },
      }}
    >
      <text style={{ color: accent ? tokens.accent : tokens.fg.primary, fontSize: tokens.font.chrome, flexGrow: 1 }}>
        {label}
      </text>
      <text style={{ color: tokens.fg.muted, fontSize: tokens.font.chip, flexShrink: 0 }}>{hint}</text>
    </div>
  );

  return (
    <anchored
      testId="ai-settings"
      anchor="topCenter"
      position={placement}
      style={{
        width: WIDTH,
        display: 'flex', // REQUIRED or children are blocks
        flexDirection: 'column',
        backgroundColor: tokens.bg.overlay,
        borderRadius: tokens.radius.panel,
        borderWidth: tokens.border.width,
        borderColor: tokens.border.glass,
        overflow: 'hidden',
      }}
    >
      <div
        testId="ai-settings-body"
        onMouseDownOutside={close}
        style={{ display: 'flex', flexDirection: 'column', padding: tokens.space.xl, gap: tokens.space.md }}
      >
        <div style={{ userSelect: 'none', display: 'flex', flexDirection: 'row', alignItems: 'center', gap: tokens.space.md }}>
          <Icon name={ICONS.palette} size={tokens.icon.button} color={tokens.fg.primary} strokeWidth={1.75} />
          <text style={{ color: tokens.fg.primary, fontSize: tokens.font.paletteInput, fontWeight: 600, flexGrow: 1 }}>
            AI Settings
          </text>
        </div>
        <text testId="ai-status" style={{ color: tokens.fg.muted, fontSize: tokens.font.chrome }}>
          {aiStatusLine(ai)}
        </text>
        <text style={{ color: tokens.fg.muted, fontSize: tokens.font.chip }}>
          {`${ai.model} at ${ai.endpoint}`}
        </text>
        <text testId="ai-privacy" style={{ color: tokens.fg.muted, fontSize: tokens.font.chip }}>
          {privacyLine(ai)}
        </text>
        {row(
          'ai-provider',
          `Provider: ${providerSettingLabel(ai.providerSetting, ai.provider)}`,
          'Click: Auto → TypeSafe → OpenCode Zen',
          cycleProvider,
        )}
        <input
          testId="ai-key-input"
          autoFocus
          value={draft}
          placeholder={keyPlaceholder(ai.providerSetting)}
          style={{
            height: tokens.strip.paletteInputHeight,
            paddingLeft: tokens.space.lg,
            paddingRight: tokens.space.lg,
            borderRadius: tokens.radius.tab,
            backgroundColor: tokens.bg.glass,
            borderWidth: tokens.border.width,
            borderColor: tokens.accent,
            color: tokens.fg.primary,
            fontSize: tokens.font.chrome,
          }}
          onChange={(event) => setDraft(String(event.value ?? ''))}
          onSubmit={save}
          onKeyDown={(event) => {
            if (event.key === 'escape') close();
          }}
        />
        {note ? (
          <text testId="ai-note" style={{ color: tokens.fg.muted, fontSize: tokens.font.chip }}>
            {note}
          </text>
        ) : null}
        {row('ai-save', 'Save key', 'Enter', save, true)}
        {row(
          'ai-screen-context',
          ai.screenContext ? 'Screen text: sent with palette queries' : 'Screen text: never sent',
          ai.screenContext ? 'On · this session only unless [ai] screen_context = true' : 'Off · click to turn on for this session',
          toggleScreenContext,
        )}
        {ai.status !== 'off' ? row('ai-test', testing ? 'Testing…' : 'Test connection', '', () => void runTest()) : null}
        {ai.source === 'app'
          ? row('ai-remove', `Remove the app-stored ${providerLabel(ai.provider)} key`, '', remove)
          : null}
        {row('ai-close', 'Close', 'Esc', close)}
      </div>
    </anchored>
  );
}
