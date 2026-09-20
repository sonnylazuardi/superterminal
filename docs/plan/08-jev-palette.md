# 08 — Semantic Command Palette & Tab Jump (Jev)

Status: **implemented on branch `feat/jev-palette`** (2026‑09‑20); this document is the record of the decisions. Written 2026‑09‑20 in
the self‑interview format of [`00-grilling.md`](./00-grilling.md): each question
gets a recommended answer and is **adopted** so the task list at the end can rely
on it. Facts were looked up in the repo and measured live; the decisions are ours.
Decisions that change earlier ones are mirrored as Q54–Q57 in `00-grilling.md`.

## 0. What we are building (30 seconds)

One palette, opened with **⌘K on macOS and Ctrl+K on Windows/Linux**, that lists
Commands, Tabs and Sessions in a single list. Typing filters instantly with the
existing local fuzzy scorer. After a short pause, the query and the candidate
list go to **Jev** (TypeSafe's System One model, a typed classifier: state +
questions in, probabilities out) which picks the candidate the user *meant*, so
"kill this tab", "split below", "the frontend one" and "work" all land on the
right row even when no title contains those words. Two ways to reach a Tab, both
in the same box: **type part of its title, cwd or session name** (local, instant)
or **describe it in plain words** (Jev). Jev never blocks the palette: with no key
configured, or offline, the palette is exactly today's palette plus tab rows.

The OpenCode Zen key can be **entered and stored from inside the app** (a palette
command opens an AI Settings dialog) as well as via `config.toml`, an environment
variable, or OpenCode's own login.

Facts that shaped this plan:

| Fact | Where |
|---|---|
| Palette has two modes (`commands`, `sessions`); ⌘K/Ctrl+Shift+K opens *sessions*, ⌘⇧P/Ctrl+Shift+P opens *commands* | `packages/app/src/ui/CommandPalette.tsx`, `commands/defaults.ts` |
| `mod` = ⌘ on macOS, **Ctrl+Shift** on Linux *and Windows* (`win32` is treated like Linux) | `platform/keys.ts:86` |
| Root `onKeyDown` matches registry bindings; the grid declines exactly `passthroughShortcuts` | `ui/App.tsx:67`, `st-native/src/element.rs:1194` |
| The client knows per Surface: title, cwd, status, exit code, `hasForegroundChild`, bell, cols/rows. **No screen text reaches JS** | `state/types.ts:47` |
| No HTTP client exists anywhere in the client; config has no `[ai]`/secrets section | survey 2026‑09‑20 |
| Config is one `config.toml` parsed by zod (client) and serde (server/CLI); unknown tables warn on both sides | `config/schema.ts`, `st-config/src/sections.rs` |
| OpenCode already stores a Zen key in `~/.local/share/opencode/auth.json` under `opencode.key` on this machine | checked 2026‑09‑20 |
| Jev via OpenCode Zen: `POST https://opencode.ai/zen/v1/systemone`, model `jev-1.13`; measured **520–890 ms** per call from WSL with ~700–900 token states; $0.042 per million input tokens, output free; 255 choice options max | probe 2026‑09‑20, flaviocopes.com/jev |

---

## A. Scope

### Q1. One palette or a "smart" layer over the existing two modes?
**Recommended:** One unified palette. Rows are typed: `command`, `tab`, `session`. The
`sessions` mode survives only as a *filter* (`session.switch` opens the same palette
with the session filter on), because "one box, any noun" is the whole point and Jev
ranks best over one candidate list. Filtering by kind is implicit: a query that is
clearly a command still shows a matching tab below it.
**Adopted.** `PaletteMode` becomes `'all' | 'sessions'`; `'commands'` is folded into `'all'`.

### Q2. What does a Tab row know, and can Jev answer "the one running tests"?
**Recommended:** Be honest about the data. The client has **title, cwd, session
name, position, running/exited, busy (foreground child), bell** per Tab and nothing
else. Jev can therefore resolve "the frontend one", "the tab in ~/projects/api",
"the exited one", "the busy one", "the tab in Work", but *not* "the one running
tests" unless the shell's OSC title says so (zsh/bash default titles show
`user@host:path`; many prompts set the running command as the title, and then it
works). Screen‑content matching needs Rust‑side text and belongs to the Attention
Radar plan, not this one.
**Adopted.** Tab candidates are described to Jev as
`Tab 3 · frontend · ~/projects/app · session Work · busy`. The plan doc says so in
the user‑facing docs too, so nobody demos a query the data cannot answer.

### Q3. Natural language to Commands: what does Jev see?
**Recommended:** Add a one‑line `description` to every `CommandDefinition`
("Split the focused pane so the new pane is below it"). Jev's `choice` criteria for
a command is `title — description`; the local fuzzy scorer keeps matching titles
only, so today's behaviour is unchanged when Jev is off.
**Adopted.** Descriptions are also the seed for a future tooltip; no other use.

### Q4. Is this in tension with Q5 ("AI features out of v1")?
**Recommended:** Yes, and v1 shipped (M0–M6 done, Windows MSI 0.1.15). Supersede Q5's
"AI features" item with: AI features are allowed **as opt‑in, network‑calling,
chrome‑only** additions that never touch the cell path (I2) and degrade to the
non‑AI behaviour when unconfigured or offline.
**Adopted** as Q54 in `00-grilling.md`.

---

## B. Keybinding

### Q5. ⌘K / Ctrl+K must open the palette on macOS and Windows. What about Linux, and what about readline?
**Recommended:** Bind `palette.commands` to `{ darwin: 'cmd+k', other: 'ctrl+k' }` and
keep `mod+shift+p` as a second binding. Plain Ctrl+K is readline's kill‑to‑end‑of‑line
and a terminal keystroke on Linux/Windows; Q29 deliberately avoided plain Ctrl for
that reason. The user explicitly wants Ctrl+K, VS Code and Slack have trained that
chord, and the cost is one readline key that is recoverable with a one‑line
override: `"palette.commands" = "ctrl+shift+p"` in `[keybindings]`. Linux gets the
same as Windows (they share `other`).
**Adopted** as Q55. Documented next to the `[keybindings]` example in
`docs/config-example.toml` ("Ctrl+K is taken from the shell by default; rebind to
give it back").

### Q6. `session.switch` currently owns `mod+k`. Where does it go?
**Recommended:** Sessions live in the unified palette, so `session.switch` becomes
"open the palette with the session filter" and moves to `mod+shift+s`. `mod+shift+k`
is not free on macOS (`surface.clearScrollback`). Nothing else uses `mod+shift+s`.
**Adopted.** Old binding recorded as superseded in `05-client-app.md` §5 table.

### Q7. ⌘K while the palette is already open?
**Recommended:** Close it (toggle). Today ⌘K flips to sessions mode from inside the
palette; with one list that flip has no meaning.
**Adopted.** `palette.commands.run` dispatches `palette.toggle`.

### Q8. Does Ctrl+K reach the app when the Kitty keyboard protocol is on?
**Fact to verify, not a decision:** `handle_key` receives `passthrough` and `modes`
together (`element.rs:1194`). Task T3 adds a unit test in `st-client-core/keys.rs`
asserting `ctrl+k` in the passthrough set yields `KeyOutcome::Passthrough` with
`KITTY_KEYBOARD` set, and a Windows smoke check with Claude Code running (it
negotiates Kitty mode, see memory `claude-code-key-probe`).

---

## C. Jev call design

### Q9. Where does the HTTP call live: Rust or the Bun client?
**Recommended:** The Bun client, `packages/app/src/ai/jev.ts`. The palette is React
state, the candidates are chrome data already in the store, no cell data is
involved (I2 holds), and Bun's `fetch` works on all three platforms including the
Windows client talking to a WSL daemon. The Server stays network‑free.
**Adopted.** The adapter takes `fetch` by injection so tests never touch the network.

### Q10. Provider and endpoint?
**Recommended:** OpenCode Zen (`https://opencode.ai/zen/v1/systemone`, `jev-1.13`)
by default because the key is already in the user's OpenCode auth store. Endpoint
and model are config keys so Vercel AI Gateway, Venice or TypeSafe direct can be
swapped in without code. Zen's two error envelopes are mapped like the
hono‑sonnylab adapter: free‑tier limit → retry later, model error → disable for the
session, bad request → log.
**Adopted.**

### Q11. What exactly is sent?
**Recommended:** State is a JSON object:
```json
{ "query": "kill this tab",
  "active_tab": "Tab 2 · api · ~/projects/api · session Work",
  "candidates": [
    { "id": "cmd:tab.close",  "kind": "command", "label": "Close Tab — Close the active tab and every pane in it" },
    { "id": "tab:7",          "kind": "tab",     "label": "Tab 1 · frontend · ~/projects/app · session Work · busy" },
    { "id": "session:2",      "kind": "session", "label": "Session Work · 3 tabs · active" } ] }
```
Questions, all in one request (Jev evaluates them in parallel at no extra cost):
- `pick` — `choice`, criteria = candidate id → label. "Which candidate does the
  query ask for? Prefer a tab when the query names a place or project, a command
  when it names an action, a session when it names a session."
- `any_match` — `noul`, "Does at least one candidate satisfy the query?" Guards
  against Jev's known behaviour of spreading probability over nonsense input
  (the jev‑shell‑history dual gate).
- `kind` — `choice` over `command | tab | session | none`, used only to break ties.
Not sent: screen text, scrollback, selection, environment, the config file. Sent:
query, titles, cwds, session names, command titles/descriptions. That list goes in
the docs verbatim.
**Adopted.** Candidate count is bounded: all visible commands (~25) + tabs of the
active Session + all Sessions; 255 is Jev's ceiling and is far away. Only enabled
commands (`when`) are candidates.

### Q12. When is Jev called?
**Recommended:** After **250 ms** of no typing, when the query has ≥ 2 non‑space
characters, at most one request in flight (the previous one is aborted). A
per‑query cache (`Map<query, result>`) makes backspacing free. No call when Jev
is unconfigured, disabled, or has been marked unavailable this session.
**Adopted.** Cost at this cadence is well under a cent per day of heavy use.

### Q13. How do Jev's results merge with the local fuzzy list without the cursor jumping?
**Recommended:** Two regimes, decided by whether local fuzzy matching found anything:
1. **Local has rows** (the common, fast case). Jev may **promote** its top pick to
   row 0 only if `pick.probability ≥ 0.6` *and* `any_match ≥ 0.5` *and* the user has
   not moved the selection (`paletteIndex === 0`). Everything else keeps fuzzy order.
2. **Local has no rows** (today: "No matches"). The list is filled with Jev's
   candidates whose probability ≥ 0.1, in probability order, capped at 8, when
   `any_match ≥ 0.5`. Below that the palette shows "No matches" as before.
Enter always activates the row under the cursor at that instant; it never waits
for Jev. A promoted row carries a small ✦ glyph in the hint column so the user
can see it was a semantic pick.
**Adopted.** Thresholds are constants in `ai/palette-rank.ts`, tuned by the fixture
run in T8; the pi‑jev‑gate write‑up shows intent probabilities are strongly
bimodal, so these are expected to be easy.

### Q14. Any visible "thinking" state?
**Recommended:** A muted dot in the input's right edge while a request is in
flight, nothing else. No spinner, no probability numbers in the chrome. With
`ST_DEBUG=st:ai` the debug log prints the full answer and latency per query.
**Adopted.**

---

## D. Setup and the API key

### Q15. Where does the key come from, in what order?
**Recommended:**
1. `[ai] api_key` in `config.toml` (an explicit, hand‑written declaration wins).
2. The key **stored from the app's AI Settings dialog** (Q17), in a per‑user
   secrets file next to the Client State file: `secrets.json` in
   `stateDir()` — `%LOCALAPPDATA%\superterminal\` on Windows,
   `~/Library/Application Support/superterminal/` on macOS,
   `$XDG_STATE_HOME/superterminal/` or `~/.local/state/superterminal/` on Linux.
   Written with mode 0600 on Unix; on Windows the per‑user profile ACL is the
   protection, same as OpenCode's own store.
3. `SUPERTERMINAL_AI_API_KEY` environment variable (CI, scripts).
4. OpenCode's own auth store, `~/.local/share/opencode/auth.json` → `opencode.key`
   (zero‑setup for anyone who already logged into OpenCode Zen; on Windows also
   `%USERPROFILE%\.local\share\opencode\auth.json`, best‑effort).
Why a separate file and not Config or Client State: Config is "written by hand,
never by the program" and Client State is "what the Client remembers without the
user declaring it" (CONTEXT.md); a key the user typed into a dialog is neither. It
is a declared secret, so it gets its own file that is never printed and never
synced with anything.
Jev is **on when a key is found and `[ai] palette` is not `false`**. The key's
*source* is logged at debug level ("ai: key from config/app/env/opencode");
the key itself never appears in any log, toast, dialog or CLI output beyond its
last four characters.
**Adopted** as Q57.

### Q16. Config schema, both sides?
**Recommended:** New table:
```toml
# Optional AI ranking for the command palette (docs/plan/08-jev-palette.md).
# Sends the palette query, command titles, tab titles, working directories and
# session names to the provider. Never screen contents.
[ai]
# Key for the provider. Unset: $SUPERTERMINAL_AI_API_KEY, then OpenCode's
# own login (~/.local/share/opencode/auth.json).
# api_key = ""
# Rank palette rows with Jev. Effective only when a key is found.
palette = true
# Provider endpoint and model. The default is OpenCode Zen.
endpoint = "https://opencode.ai/zen/v1/systemone"
model = "jev-1.13"
```
Client: `AiSchema` in `config/schema.ts` (snake_case canonical, `api_key` alias
`apiKey`). Rust: `AiConfig` in `st-config/src/sections.rs` marked client‑only in
its doc comment, so `st config init` documents it and `st config check` does not
warn; the server never reads it. `example.rs` gets the section so the "comment
table covers every serialised key" test keeps passing.
**Adopted.**

### Q17. Setup UX inside the app?
**Recommended:** A palette command **"AI Settings…"** (id `ai.settings`, no default
shortcut) opens a Dialog — top centre like every Dialog, Esc dismisses, built like
`About.tsx` — with:
- **Status line**: "Jev ranking: on · key from app" / "off · no key found", the
  endpoint, and the last error if any.
- **Key field**: a gpuix `<input>` with placeholder "OpenCode Zen API key". The
  gpuix input binds ⌘V and Ctrl+V to its own Paste action on every desktop build
  (`vendor/gpuix/packages/native/src/custom_elements/input.rs:223`), so pasting a
  key works without any change to `edit.paste`; the root `onKeyDown` never sees the
  chord because the input consumes it. The input has **no masked mode**, so the key
  is visible while being typed and the field is emptied on save; afterwards the
  status line shows only `…` + the last four characters.
- **Save** (Enter or button): validates the shape (non‑empty, no whitespace),
  writes `secrets.json`, re‑resolves the key source, and runs one tiny Jev call as
  a **connection test** (a `noul` over a one‑word state); the result ("OK · 612 ms"
  or the mapped error) is shown in the status line. Failure still saves the key.
- **Remove key**: deletes the app‑stored key; the source falls through to
  env/OpenCode.
- **Open config folder** is not offered (nothing to edit by hand for this flow).
When Jev is off and the local list is empty, the palette's "No matches" row reads
"No matches · AI ranking is off — run AI Settings…" and Enter on it opens the
dialog. "AI: Show Status" from the earlier draft is folded into this dialog.
Non‑goals: OAuth, a browser login, encrypting the file (OpenCode does not either).
**Adopted.**

### Q17a. Who owns the stored key at runtime?
**Recommended:** A small `ai/key-store.ts` module (read/write/delete `secrets.json`,
injected fs) and an `AiState` slice in the store: `{ source, last4, endpoint,
status: 'off'|'ready'|'disabled', lastError }`. The dialog and the palette read the
slice; only `key-store.ts` touches the file. No key string is kept in React state
after save — the adapter holds it in module scope.
**Adopted.**

### Q18. Windows specifics?
**Recommended:** The Windows client reads `%USERPROFILE%\.config\superterminal\config.toml`
(`configPath` uses `HOME || homedir()`), the same file the docs already point
Windows users at, and stores the app‑entered key in
`%LOCALAPPDATA%\superterminal\secrets.json` beside `client.json`. The in‑app
dialog is the expected path for Windows users, since most will not have OpenCode
installed on the Windows side. `fetch` runs in the Windows process and honours the system proxy
through Bun. Ctrl+K arrives as `ctrl-k` in `passthroughShortcuts` exactly like
`ctrl-shift-t` does today. Testing follows memory `windows-client-testing`
(plain‑copy checkout, driver script, never touch the user's window).
**Adopted.**

---

## E. Failure and privacy

### Q19. Network down, key invalid, Zen rate‑limits?
**Recommended:** Silent degradation. Timeout 2.5 s per request; any error marks
the request failed and the palette shows local results only. A 401/403 or a Zen
`ModelError` disables Jev for the rest of the session and records the message for
"AI: Show Status". A 429 backs off 30 s. No toast for any of these: a palette that
sometimes says "AI unavailable" while you type would be worse than one that is
quietly local.
**Adopted.**

### Q20. Privacy statement?
**Recommended:** One paragraph in `docs/config-example.toml` above `[ai]` and in
README "What works": exactly what is sent (Q11), that it is off without a key, and
that cwds can contain project names. No telemetry of our own.
**Adopted.**

---

## F. Tests and acceptance

### Q21. What is unit‑tested without a network or GPU?
- `ai/jev.ts`: request body shape, header, model/endpoint from config, error
  envelope mapping, abort of a superseded request, stale‑response discard.
- `ai/palette-rank.ts` (pure): regime 1 promotion rules, regime 2 fill, the
  `paletteIndex !== 0` guard, thresholds, cap of 8.
- `ai/key-source.ts`: precedence config → app store → env → opencode auth.json,
  malformed file tolerated, never returns the key in a log string.
- `ai/key-store.ts`: round‑trip through an injected fs, 0600 on Unix, delete,
  corrupt file yields "no key" with a warning, `last4` derivation.
- `ui/ai-settings.ts` (pure part of the dialog): key shape validation, status
  line text for every source/status combination, connection‑test result mapping.
- `config/schema.ts`: `[ai]` parses, `api_key`/`apiKey` alias, unknown key warns.
- `commands/registry.test.ts`: `palette.commands` resolves to `cmd-k` on darwin and
  `ctrl-k` on linux and win32; `session.switch` is `mod+shift+s`; `passthroughShortcuts`
  contains them; override to `ctrl+shift+p` works.
- `st-client-core` keys: `ctrl+k` in the passthrough set is declined with Kitty mode on.
- `st-config`: `[ai]` round‑trips, example generator covers it, `st config check`
  is silent on a file with `[ai]`.
**Adopted.**

### Q22. How are the semantic thresholds validated?
**Recommended:** A fixture file `packages/app/src/ai/fixtures/palette-queries.json`
with ~20 (workspace, query, expected id) triples, and a script
`scripts/jev-palette-probe.ts` that runs them live and prints a table of pick,
probability, `any_match` and latency. It is run by hand (needs a key) before the
thresholds are frozen and again before release; the numbers go into this document.
**Adopted.** Starting fixtures:

| Workspace (tabs) | Query | Expect |
|---|---|---|
| api, frontend, docs | the frontend one | tab frontend |
| api, frontend, docs | kill this tab | Close Tab |
| api, frontend, docs | split below | Split Down |
| api, frontend, docs | new pane on the right | Split Right |
| sessions Work, Home | work | session Work |
| api (exited), frontend | the dead one | tab api |
| api (busy), frontend | the busy one | tab api |
| api, frontend | ~/projects/docs | none (any_match low) |
| any | bigger text | Make Text Bigger |
| any | asdfgh | none |

**Live run, 2026‑09‑20** (`bun scripts/jev-palette-probe.ts`, key from the OpenCode
login, `jev-1.13` via OpenCode Zen from WSL2): **15/15 pass** at the frozen
thresholds (promote ≥ 0.6, any_match ≥ 0.5), latency **p50 593 ms, p95 961 ms**.

| Query | Expect | Pick | p | any_match |
|---|---|---|---|---|
| the frontend one | tab frontend | ✓ | 1.00 | 0.89 |
| kill this tab | Close Tab | ✓ | 0.98 | 0.97 |
| split below | Split Down | ✓ | 1.00 | 0.95 |
| new pane on the right | Split Right | ✓ | 1.00 | 0.98 |
| work | session Work | ✓ | 0.80 | 0.58 |
| go home | session Home | ✓ | 0.71 | 0.59 |
| the dead one | tab docs (exited) | ✓ | 0.96 | 0.59 |
| the busy one | tab api (busy) | ✓ | 1.00 | 0.63 |
| bigger text | Make Text Bigger | ✓ | 1.00 | 0.98 |
| make the font smaller | Make Text Smaller | ✓ | 1.00 | 0.98 |
| the docs tab | tab docs | ✓ | 0.98 | 0.67 |
| open another shell | New Tab | ✓ | 0.99 | 0.94 |
| set up jev | AI Settings… | ✓ | 1.00 | 0.94 |
| asdfgh | none | any_match 0.08 | 0.66 | 0.08 |
| ~/projects/nothing-here | none | any_match 0.32 | 0.41 | 0.32 |

Two lessons from the run: (1) the `any_match` gate is what makes gibberish safe —
`pick` still hands 0.66 to some tab for "asdfgh", so the choice alone must never
fill the list; (2) session and state queries ("work", "the dead one") sit at
0.55–0.60 on `any_match`, close to the 0.5 gate, so the gate must not be raised
without re‑running the fixtures. "set up jev" failed on the first run because the
command's description did not say "Jev"; descriptions are part of the contract.

### Q23. Definition of done?
1. ⌘K on macOS and Ctrl+K on Windows open the unified palette; ⌘⇧P/Ctrl+Shift+P still do.
2. With no key: palette lists commands, tabs and sessions with fuzzy matching; no network call is made (asserted by the injected fetch).
3. With a key: every fixture in Q22 passes at the frozen thresholds; p95 latency recorded.
4. Enter never waits; the selection never moves under the cursor once the user has moved it.
5. `st config init` documents `[ai]`; `st config check` and the client warn on nothing for the example file.
5a. AI Settings… accepts a pasted key on macOS (⌘V) and Windows (Ctrl+V), saves it to `secrets.json`, shows only the last four characters afterwards, runs the connection test, and Remove key falls through to the next source.
6. `just check` (fmt, typecheck, cargo test, bun test) is green; Windows smoke per memory.

---

## G. Task list (each ≤ 1 day, with acceptance)

| Id | Task | Acceptance |
|---|---|---|
| T1 | Add Q54–Q57 to `00-grilling.md`; update `05-client-app.md` §4/§5 table (⌘K → palette, `session.switch` → `mod+shift+s`). | Docs agree with this plan. |
| T2 | Registry: rebind `palette.commands` to `{darwin:'cmd+k', other:'ctrl+k'}` + `mod+shift+p`; `session.switch` → `mod+shift+s`; toggle semantics; `description` on every `CommandDefinition`. | `registry.test.ts` cases in Q21 pass on all three platforms. |
| T3 | Verify Ctrl+K passthrough under Kitty mode in `st-client-core` (unit test) and on Windows with Claude Code running. | Test green; note in `docs/DEV.md`. |
| T4 | Unified palette: `PaletteMode = 'all' \| 'sessions'`, tab rows (`displayTitle`, cwd hint, ✓ for active), session rows, kind chip in the hint column. Pure row‑building in `ui/palette-rows.ts` so it is testable without gpuix. | Local‑only palette lists all three kinds; existing session "New Session ‹q›" row preserved. |
| T5 | Config: `[ai]` in zod (`schema.ts`), in Rust (`sections.rs`, `example.rs`), `docs/config-example.toml` regenerated; `ai/key-source.ts` with the three‑step lookup. | Q21 config tests; `cargo test -p st-config` green. |
| T6 | `ai/jev.ts` adapter: injected fetch, abort, timeout, error mapping, session disable/backoff state. | Q21 adapter tests. |
| T6b | `ai/key-store.ts` (secrets.json read/write/delete, injected fs), `AiState` slice + reducer actions (`ai.setStatus`), key‑source resolution at bootstrap. | Q21 key‑store tests; bootstrap logs the source, never the key. |
| T7 | `ai/palette-rank.ts` merge logic + `usePaletteRanking` hook (debounce 250 ms, cache, single in‑flight) wired into `CommandPalette.tsx`; ✦ marker; in‑flight dot; "AI ranking is off — run AI Settings…" empty row. | Manual: queries from Q22 behave; no cursor jump after moving selection. |
| T7b | `ui/AiSettings.tsx` Dialog + `ai.settings` command: status line, key input (paste verified on macOS and Windows), Save with connection test, Remove key; one Dialog at a time with About/palette. | Q23 item 5a on Linux/WSLg and Windows. |
| T8 | `scripts/jev-palette-probe.ts` + fixtures; run live, freeze thresholds, paste the table into this doc. | Every fixture passes; p50/p95 recorded. |
| T9 | Docs: README "What works", privacy paragraph, Ctrl+K/readline note, Windows config path. | Reviewed. |
| T10 | Windows smoke (memory `windows-client-testing`): Ctrl+K, tab jump, no key → local only. Bump nothing; release is a separate decision. | Driver script output pasted in PR. |

Order: T1 → T2 → T3 ∥ T4 → T5 → T6 → T6b → T7 ∥ T7b → T8 → T9 → T10.

---

## G2. Screen-content search (added 2026‑09‑20, same branch)

The first cut deliberately sent no screen text, and §H listed that as the main
limitation. It is now implemented as an extension of the same palette, because
the two questions users actually ask — "the tab about the baby tracker", "the
one showing memory" — are answered by what is *on screen*, not by the title.

### Q24. Where does the text come from?
**Recommended:** a new Control Plane request, `surface.screen_text`, answered by
the Server from its own authoritative grid. Not the Replica: only the visible
Tab has a Replica in the Client, and the whole point is to search the Tabs you
cannot see. Invariant I2 is untouched — this is chrome data over the Control
Plane, not cell data through the render path.
**Adopted.** Shape: `{ surfaces: SurfaceId[], max_rows?: u16 }` →
`{ screens: [{ surface, lines: string[] }] }`, blank rows dropped, last 40 rows
by default, unknown ids silently omitted so one stale id never fails a batch.

### Q25. Viewport or scrollback?
**Recommended:** viewport only. Scrollback multiplies the payload and drags in
content that no longer describes what the Tab is doing; the bottom of the screen
is what the Tab is *about* right now. The request takes a row count, so a range
can be added later without a protocol change.
**Adopted.**

### Q26. When is it fetched?
**Recommended:** once, when the palette opens, for every Surface of every Tab in
the active Session, in a single request; cached until the palette closes. It is
a local socket round trip against data the Server already holds. The palette
renders immediately and re-ranks when the map lands, so a slow answer never
delays the first keystroke. A daemon without the request fails the call and the
palette silently behaves as it did before.
**Adopted.**

### Q27. Does searching screen text need an API key?
**Recommended:** no, and this matters. The literal match ("baby", "ram") runs
locally over the fetched text: every word of the query must appear somewhere in
the Tab's screen. A Tab matched this way shows the matching line as evidence in
its hint. Only the *semantic* form ("the one showing memory usage") needs Jev,
and only then is an excerpt included in what the model reads.
**Adopted.** Title matches still outrank content matches, so typing a Tab's name
never gets displaced by a coincidence deeper in someone's screen.

### Q28. Privacy: screen text is the most sensitive thing this app holds.
**Recommended:** the local search is always on — nothing leaves the machine.
Sending excerpts to the provider is a separate opt-in, `[ai] screen_context`,
**default false**, flippable for the session in AI Settings…. Before any text
leaves, a redaction pass masks bearer tokens, `sk-`/`ghp_`/`xoxb-`-style keys,
AWS ids, private-key blocks, `password=`/`token=`/`secret=` assignments and long
hex/base64 runs. Excerpts are capped (the last ~15 non-blank lines, ~600
characters per Surface) so a full workspace stays a few thousand tokens.
**Adopted.** The dialog and `config-example.toml` both say plainly what is sent.

### Q29. What it still cannot do
Aggregate questions ("how many split tabs are there") are not palette queries:
the palette picks a row, and Jev is unreliable at counting. Pane counts are
already in each row's label, so "the tab with 3 panes" works; a count across the
workspace does not, and should not be faked.

## H. Out of scope (hooks left)

- ~~**Screen‑content tab search**~~ — done, see §G2. What remains out: scrollback
  search, and a per‑Surface `attention` summary (the Attention Radar idea), which
  would become one more field in the tab candidate label here.
- **OAuth / browser login, key encryption, masked input**: the dialog takes a pasted key and stores it in plain JSON with user‑only permissions, like OpenCode.
- **Palette actions with arguments** ("rename session to X"): Jev cannot extract
  free text, only choose; would need a second stage.
- **Per‑keystroke ranking** without debounce: Zen latency is 0.5–0.9 s today; revisit
  if a ~100 ms provider path is configured.

## Shared understanding reached

The palette becomes one list of three nouns, opened by ⌘K/Ctrl+K, that is exactly as
fast and offline‑capable as today, and that quietly gets smarter when a Jev key is
present. The Server, the wire protocol and the cell path are untouched.
