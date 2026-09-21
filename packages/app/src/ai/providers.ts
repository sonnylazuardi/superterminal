/**
 * The System One providers the client knows how to call.
 *
 * Jev is TypeSafe's model; it is reachable two ways that speak the same
 * request/response body:
 *   - TypeSafe direct (`api.typesafe.ai`): about half the latency of the
 *     passthrough (≈ 280 ms warm vs ≈ 600 ms), keys from their console.
 *   - OpenCode Zen's `/v1/systemone` passthrough: works with an OpenCode
 *     login many users already have, so it needs no new key.
 *
 * The two disagree on model ids (Zen's `jev-1.13` is an unknown model at
 * TypeSafe, which wants the full `jev-1.13.0`), so the endpoint and model are
 * a property of the provider, not of the config: config may still override
 * either, but by default switching provider switches both. TypeSafe pins the
 * versioned id rather than `jev-latest` because the palette thresholds were
 * tuned against 1.13.
 *
 * `auto` prefers the direct endpoint when both keys exist (PROVIDER_ORDER).
 */

import type { ProviderId } from '../state/types.js';

export interface ProviderPreset {
  id: ProviderId;
  label: string;
  endpoint: string;
  model: string;
  /** Provider-specific env var, checked before the generic one. */
  envKey: string | null;
  /** Whether OpenCode's own login (`auth.json`) holds a key for it. */
  opencodeLogin: boolean;
  /** How its keys start, for routing a pasted key under `auto`. */
  keyPrefix: string | null;
  /** Where to get a key. */
  consoleUrl: string;
}

export const PROVIDERS: Record<ProviderId, ProviderPreset> = {
  typesafe: {
    id: 'typesafe',
    label: 'TypeSafe',
    endpoint: 'https://api.typesafe.ai/v1/systemone',
    model: 'jev-1.13.0',
    envKey: 'TYPESAFE_API_KEY',
    opencodeLogin: false,
    keyPrefix: 'apikey_',
    consoleUrl: 'https://console.typesafe.ai/keys',
  },
  zen: {
    id: 'zen',
    label: 'OpenCode Zen',
    endpoint: 'https://opencode.ai/zen/v1/systemone',
    model: 'jev-1.13',
    envKey: null,
    opencodeLogin: true,
    keyPrefix: null,
    consoleUrl: 'https://opencode.ai/zen',
  },
};

/** Preference order under `auto`: the direct endpoint first. */
export const PROVIDER_ORDER: readonly ProviderId[] = ['typesafe', 'zen'];

/** Which provider a key belongs to, judged by its shape alone. */
export function detectProvider(key: string): ProviderId {
  const prefix = PROVIDERS.typesafe.keyPrefix;
  return prefix !== null && key.trim().startsWith(prefix) ? 'typesafe' : 'zen';
}

export function providerLabel(id: ProviderId | null): string {
  return id ? PROVIDERS[id].label : 'none';
}

/** The provider's endpoint and model, with any non-blank config override applied. */
export function resolveTarget(
  provider: ProviderId,
  overrides: { endpoint?: string | undefined; model?: string | undefined },
): { endpoint: string; model: string } {
  const preset = PROVIDERS[provider];
  const endpoint = overrides.endpoint?.trim();
  const model = overrides.model?.trim();
  return { endpoint: endpoint || preset.endpoint, model: model || preset.model };
}
