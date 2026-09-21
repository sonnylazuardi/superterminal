# 09 — TypeSafe's direct Jev endpoint (a second provider)

Status: **implemented on branch `feat/jev-typesafe-endpoint`** (2026‑09‑21).
Self‑interview format of [`00-grilling.md`](./00-grilling.md): each question gets
a recommended answer and is **adopted**. Builds on
[`08-jev-palette.md`](./08-jev-palette.md), which shipped Jev ranking through
OpenCode Zen only.

## 0. What changes (30 seconds)

Jev is TypeSafe's model; OpenCode Zen resells it through a passthrough. Calling
TypeSafe directly is about **twice as fast** and needs no OpenCode account. The
palette now talks to either provider. `provider = "auto"` (the default) uses
TypeSafe when a TypeSafe key is found and falls back to Zen, so existing setups
keep working and a pasted `apikey_…` key just works.

Facts, all checked live on 2026‑09‑21 or read from docs.typesafe.ai:

| Fact | Zen passthrough | TypeSafe direct |
|---|---|---|
| Endpoint | `https://opencode.ai/zen/v1/systemone` | `https://api.typesafe.ai/v1/systemone` |
| Auth | `Authorization: Bearer <zen key>` | `Authorization: Bearer apikey_…` |
| Key from | OpenCode login / opencode.ai | console.typesafe.ai/keys, env `TYPESAFE_API_KEY` |
| Model id | `jev-1.13` | `jev-1.13.0`, aliases `jev-latest`, `jev-preview`; **`jev-1.13` → 400 "Unknown model"** |
| Warm latency (WSL2) | ≈ 550–650 ms | ≈ 270–300 ms |
| Error body | HTTP 200 `{"type":"error","error":{"type":"ModelError"…}}` | `{"detail":{"error_type":"authentication_error","message":…}}` or `{"detail":"…"}` |
| Rate limit | free‑tier text | 429 (+ `Retry-After`), 529 overloaded |
| Request id | — | `x-typesafe-request-id` header |

---

## A. Shape

### Q1. A "provider" concept, or just point `endpoint` at TypeSafe?
**Recommended:** a provider concept. Pointing `endpoint` at TypeSafe with the old
default model fails outright (`jev-1.13` is unknown there), the two services use
different keys, different key sources and different error bodies. A preset per
provider carries endpoint, model, env var, key prefix and whether OpenCode's login
applies; `endpoint`/`model` stay as optional overrides for proxies and gateways.
**Adopted.** `ai/providers.ts` holds the presets.

### Q2. Which model id for TypeSafe?
**Recommended:** pin **`jev-1.13.0`**, not `jev-latest`. The palette thresholds
(promote ≥ 0.6, any_match ≥ 0.5) were tuned against 1.13, and TypeSafe's own docs
say to pin the versioned id when you have tuned confidence thresholds. The dialog
shows the versioned id the service actually answered with, so a future alias move
is visible.
**Adopted.** Override with `[ai] model = "jev-latest"` if you want the alias.

### Q3. How is the provider chosen?
**Recommended:** `[ai] provider = "auto" | "typesafe" | "zen"`, default `auto`.
Auto walks the existing source precedence (config → app → env → OpenCode login)
and the first key found decides the provider; where a source can hold both,
TypeSafe is tried first because it is the faster, direct path. A key's provider
is known from where it was stored, or from its prefix (`apikey_` is TypeSafe)
when it arrives through a provider‑neutral channel (`api_key`,
`$SUPERTERMINAL_AI_API_KEY`).
**Adopted.** Existing Zen users see no change: no TypeSafe key, so auto → Zen.

### Q4. Where does the dialog's provider choice live?
**Recommended:** **Client State** (`aiProvider` in `client.json`), not Config and
not the secrets file. It is a choice the user made in the UI without declaring it
in config.toml, exactly like the sidebar width — and Config is never written by
the program. Client State wins over Config (ADR 0008), so `[ai] provider` seeds
the first run and the dialog overrides it afterwards.
**Adopted.**

## B. Keys

### Q5. One stored key or one per provider?
**Recommended:** one per provider:
`secrets.json` → `{"ai":{"keys":{"typesafe":"apikey_…","zen":"…"}}}`. Switching
providers must not destroy the other key. The legacy `{"ai":{"api_key"}}` shape is
read as the Zen key and migrated on the next write.
**Adopted.**

### Q6. Key sources per provider?
| Source | TypeSafe | Zen |
|---|---|---|
| `[ai] api_key` | if provider is typesafe, or auto + `apikey_` | otherwise |
| App store (`secrets.json`) | `keys.typesafe` | `keys.zen` |
| Environment | `TYPESAFE_API_KEY`, then `SUPERTERMINAL_AI_API_KEY` (prefix‑detected) | `SUPERTERMINAL_AI_API_KEY` |
| OpenCode login | — | `~/.local/share/opencode/auth.json` |
**Adopted.** An explicit provider only ever uses its own column.

## C. Errors

### Q7. How do TypeSafe's errors map onto the existing failure policy (08 Q19)?
| TypeSafe response | Kind | Effect |
|---|---|---|
| 401 `authentication_error`, 403 | `auth` | disabled for the session, shown in the dialog |
| 400 `api_usage_error` "Unknown model" | `model` | disabled for the session — without this mapping every keystroke would retry a request that can never succeed |
| 400/422 `{"detail":"…"}` | `bad_request` | logged |
| 429, 529 | `rate_limit` | back off for `Retry-After` / `retry-after-ms`, else 30 s |
| network / timeout | `network` | palette stays local, silent |
**Adopted.** Zen's HTTP‑200 envelope keeps working as before.

## D. UI

### Q8. What does AI Settings… show?
**Recommended:** a provider row that cycles **Auto → TypeSafe → OpenCode Zen**
(hint: what Auto resolved to), a key field whose placeholder names the target
provider, Save → stores under that provider and runs a connection test that
reports provider, versioned model and latency, and Remove for the provider in use.
The status line names the provider: `AI ranking: on · TypeSafe · key from this app
· key …3a2f · last call 290 ms`.
**Adopted.**

## E. Tests and acceptance

- Unit: presets, prefix detection, target overrides; per‑provider key store incl.
  legacy migration; source precedence for auto and explicit providers; every
  TypeSafe error body; the service switching endpoint/model on `setProvider`.
- Live: `bun scripts/jev-palette-probe.ts --provider typesafe` and
  `--provider zen` — the 15 fixtures must pass on both at the same thresholds.
- Dev run: Windows client with a TypeSafe key stored through AI Settings… ranks
  palette queries; the dialog reports TypeSafe, `jev-1.13.0` and the latency.

**Live run, 2026‑09‑21** (`bun scripts/jev-palette-probe.ts --provider …`, WSL2,
thresholds promote ≥ 0.6 / any_match ≥ 0.5):

| Provider | Model | Result | p50 | p95 |
|---|---|---|---|---|
| TypeSafe direct | `jev-1.13.0` | 15/15 | 290 ms | 749 ms (the cold first call; warm 263–361 ms) |
| OpenCode Zen | `jev-1.13` | 15/15 | 654 ms | 999 ms |

Both providers picked the same row on every query with close probabilities, so
the 1.13 thresholds carry over to `jev-1.13.0` unchanged.

## F. Out of scope

- Using TypeSafe's SDK (`@typesafe-ai/sdk`): the adapter is ~150 lines of `fetch`
  with injected I/O; the SDK would add retries we deliberately do not want on a
  keystroke path, and a dependency.
- Automatic failover between providers on error: a disabled provider is shown in
  the dialog; switching is one click. Silent failover would hide a broken key.
- Vercel AI Gateway / Venice presets: `endpoint` + `model` overrides already
  cover them; add presets when someone needs the key plumbing.
