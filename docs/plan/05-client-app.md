# 05 — Client app (`packages/app`)

> **Addendum (00-grilling §F):** Q44 supersedes §4's "keep N=4 tabs mounted with display:none" — mount only the visible `<terminal-grid>`; warm Replicas live in Rust. Q43: selection/scroll are not sent by the app. Q46: config schema is shared with the Rust `st-config` crate via fixtures. Q48: default Session `Default`; `tab.set_active` exists; `has_foreground_child`/`cwd` arrive in `SurfaceStatus`.

Scope: the Bun + React process that draws chrome and hosts one `<terminal-grid>` per tab. It never touches cell data (Q10, Q13). Everything here is a plan; snippets are illustrative. Decisions from `00-grilling.md` are taken as given; anything unresolved is under **Open questions**.

Related: `02-protocol.md` (control-plane messages), `03-server.md` (socket paths, lifecycle), `04-client-native.md` (`<terminal-grid>` props/events, data plane), `06-testing-perf.md`.

---

## 1. App bootstrap

Entry: `packages/app/src/app.tsx`. Startup is a straight line; each step has one owner module.

```
parse argv ──▶ load config ──▶ ensure server ──▶ connect control plane
   │  (--version, --socket, --foreground-server, --no-spawn)
   └──▶ pick window options ──▶ render(<App/>) ──▶ store gets Hello + WorkspaceSnapshot
```

**Config (Q34).** `~/.config/superterminal/config.toml` (respecting `$XDG_CONFIG_HOME`). Verified on Bun 1.4.0: `import config from './x.toml'` works and `Bun.TOML.parse(text)` exists (`Bun.TOML` exposes `parse`/`stringify`). We use **`Bun.TOML.parse`** at runtime, not a static import: the static form is resolved and inlined at bundle time, so it cannot read a user file whose path is only known at runtime. The parsed object is validated into a typed `Config` with zod (already a dependency of `@gpuix/react`; declared explicitly in `packages/app`). Unknown keys warn to stderr and are ignored; a missing file yields defaults. Only the client-relevant subset is typed here; the server reads the same file for `shell`/`env`.

```ts
// packages/app/src/config/schema.ts
export const ConfigSchema = z.object({
  font: z.object({ family: z.string().optional(), size: z.number().default(13),
                   lineHeight: z.number().default(1.2) }).default({}),
  window: z.object({
    background: z.enum(['auto', 'blurred', 'transparent', 'opaque']).default('auto'),
    verticalTabs: z.boolean().default(false),
  }).default({}),
  theme: z.record(z.string()).default({}),           // palette overrides, 04 defines keys
  keybindings: z.record(z.string(), z.string()).default({}), // commandId -> "mod+shift+t"
});
export type Config = z.infer<typeof ConfigSchema>;
```

**Ensure server (Q30).** `control/ensureServer.ts`:
1. Socket path: `$SUPERTERMINAL_SOCKET` if set, else the default from `03-server.md` (`$XDG_RUNTIME_DIR/superterminal/control.sock`).
2. Probe: `Bun.connect({ unix })` with a 500 ms timeout. Success → keep that socket as the control connection (no double connect).
3. Failure (ENOENT/ECONNREFUSED) and `--no-spawn` absent → locate `superterminald`: `$SUPERTERMINAL_SERVER`, then beside the client binary (`dirname(process.execPath)`), then `$PATH`. Spawn `Bun.spawn([bin], { stdio: ['ignore','ignore','ignore'], detached: true })` (Bun 1.4 supports `detached`) then `proc.unref()` so the client can exit without killing it. The server handles stale-socket cleanup and the lockfile itself (Q30).
4. Retry connect every 250 ms for 3 s; give up → render `<App>` anyway with `connection = 'failed'` and a `<Banner>` offering **Retry** / **Start server**. The window must appear even when the server is broken; the user needs to see *why*.

**Window options (Q28).** `platform/windowOptions.ts` returns the `render()` options:

| | macOS | Linux |
|---|---|---|
| `windowBackground` | `'blurred'` | `resolveLinuxBackground(config)` |
| `titlebarTransparent` | `true` | `false` (no traffic lights; strip starts at top) |
| `trafficLightX/Y` | `18, 18` | — |
| `title`/`appName` | `'superterminal'` | same |
| `minWidth × minHeight` | `480 × 320` | same |
| `focus` | `true` | `true` (`false` is ignored on Linux anyway) |

`resolveLinuxBackground`: `config.window.background` if not `'auto'`; else `'opaque'` when running under WSLg (`/proc/version` contains `microsoft` — the RDP compositor mishandles alpha), else `'transparent'` when `WAYLAND_DISPLAY` is set, else `'opaque'` (X11 compositor presence is not cheaply probeable; users opt in via config). `'blurred'` on Linux is treated as `'transparent'`. The choice is logged at startup.

`render(<App/>, opts)` is called once; there is no re-render path for window options (hot-reload of config is deferred, Q34).

---

## 2. Control-plane client (`src/control/client.ts`)

One `Bun.connect({ unix })` socket, newline-delimited JSON (Q14). The module is transport-only: it knows framing, correlation, and reconnection; it knows nothing about tabs.

```ts
export interface ControlClient {
  request<M extends RequestType>(type: M, params: ReqParams<M>, opts?: { timeoutMs?: number })
    : Promise<ResOk<M>>;                     // rejects with ControlError{code,message} or TimeoutError
  on(listener: (ev: ServerEvent) => void): () => void;
  readonly state: 'connecting' | 'connected' | 'reconnecting' | 'closed';
  close(): void;
}
```

- **Framing.** The `data` handler appends to a `Uint8Array` buffer, splits on `\n`, `JSON.parse`s each complete line. Lines > 4 MiB abort the connection (defensive; control messages are small).
- **Correlation.** Every request gets `id: number` (monotonic). A `Map<id, {resolve, reject, timer}>` holds pending calls; default timeout 5 s (`CreateSurface` 15 s — spawning a shell can be slow on cold disks). Responses without a matching id are logged and dropped.
- **Events.** Messages with no `id` are server pushes (`WorkspaceChanged`, `SurfaceStatus`, `SurfaceTitle`, `Bell`, `ServerShutdown`…). They are dispatched to listeners synchronously in arrival order; the store is the only listener in production.
- **Handshake.** On `open`, send `Hello{proto_version, build_id}` first; the reply carries the server's version and the initial `WorkspaceSnapshot`. A major-version refusal (Q31) resolves to `state='closed'` with a `VersionMismatch` error the store shows as a banner.
- **Reconnect.** On `close`/`error` while not intentionally closing: exponential backoff 250 ms → 4 s, and after 3 consecutive failures call `ensureServer` again (the server may have exited idle). Pending requests are rejected with `Disconnected`. The data plane reconnects independently inside the native module (04); ordering is preserved because the store only renders `<terminal-grid>` for surfaces present in the latest snapshot.

**Types: `packages/protocol-ts`.** The TS mirror of `02-protocol.md` is **generated from `st-proto` with `ts-rs`**, not hand-written. Reason over `typeshare`: `ts-rs` honours the serde attributes (`tag`/`content`/`rename_all`) the JSON codec already uses, so emitted unions match the wire format by construction, and it needs no extra CLI binary. `just proto-ts` runs `cargo test -p st-proto --features ts-export`, which writes `packages/protocol-ts/src/generated/*.ts`; CI fails if the generated files differ from the committed ones. Hand-written code in `protocol-ts` is limited to `index.ts` (re-exports, `RequestType`/`ReqParams`/`ResOk` helper types) and `version.ts` (`PROTO_VERSION`, also generated).

---

## 3. State management

A single **`WorkspaceStore`** built on `useSyncExternalStore` with a hand-rolled store (~60 lines): `getState()`, `subscribe()`, `dispatch(action)`. No Zustand/Redux; the projection is small and the reducer must be trivially unit-testable.

```ts
export interface WorkspaceState {
  connection: { status: 'connecting'|'connected'|'reconnecting'|'failed'|'mismatch';
                serverVersion?: string; error?: string };
  sessions: Record<SessionId, { id: SessionId; name: string; tabIds: TabId[] }>;
  sessionOrder: SessionId[];
  activeSessionId: SessionId | null;
  tabs: Record<TabId, { id: TabId; sessionId: SessionId; surfaceId: SurfaceId }>;
  activeTabBySession: Record<SessionId, TabId>;
  surfaces: Record<SurfaceId, { id: SurfaceId; title: string; cwd: string;
             status: 'starting'|'running'|'exited'; exitCode?: number;
             hasForegroundChild: boolean; bell: boolean }>;
  ui: { paletteOpen: boolean; paletteMode: 'commands'|'sessions'; paletteQuery: string;
        verticalTabs: boolean; renamingSessionId: SessionId | null;
        window: { width: number; height: number }; toasts: Toast[] };
}
```

Two reducer families, both pure and tested in isolation:
- `applyServerEvent(state, ev)` — the server is authoritative for `sessions/tabs/surfaces/active*` (Q17). `WorkspaceSnapshot` replaces the whole projection; incremental events patch it. The client never mutates these speculatively; a tab click sends `ActivateTab` and waits for the echo (one local round trip, < 1 ms — optimistic updates are not worth the reconciliation code).
- `applyUiAction(state, action)` — client-only state (`ui`), never sent to the server.

Selectors (`selectActiveTabs`, `selectActiveSurface`, `selectMountedSurfaceIds`) are plain functions memoised per state reference. Components subscribe via `useWorkspace(selector)`.

---

## 4. Component tree

```
<App>                                   store provider, root onKeyDown → dispatchKey()
 ├─ <TitleBarSpacer/>                   macOS: paddingTop 58 for traffic lights; Linux: 0
 ├─ <Frame orientation=horizontal|vertical>
 │   ├─ <TabStrip>                      row (or column when verticalTabs)
 │   │   ├─ <SessionChip/>              active session name; click → palette 'sessions' mode
 │   │   ├─ <Tab/> × N                  title, exited badge, close on hover; drag-reorder deferred
 │   │   └─ <NewTabButton/>
 │   └─ <SurfaceHost>                   flex:1; one <terminal-grid> per mounted tab
 │       └─ <terminal-grid surfaceId=…/> × ≤4   (04-client-native.md props/events)
 ├─ <CommandPalette/>                   <anchored> overlay + <input> + list (commands | sessions)
 ├─ <Banner/>                           disconnected / version mismatch / server restarting
 └─ <StatusToasts/>                     bell, copy confirmation; auto-dismiss 2.5 s
```

**`<SurfaceHost>` mounting policy — decision: keep the last N = 4 tabs mounted, hidden with `display: 'none'`; unmount beyond that (MRU).** Reasoning against pure unmount-and-reattach: Q27's target is attach-to-first-paint < 100 ms *warm*, but a re-attach costs a `Snapshot` (200×100 grid + style table ≈ 100–200 KB), a full re-shape of every visible row, and it discards the Replica's cached history pages and scroll position (view_state comes back from the server, but the rows to render at that offset must be re-fetched). That is a visible blank frame on every tab switch. Reasoning against keeping everything mounted: 40 replicas mean tens of MB of history/shaping caches and 40 delta subscriptions. N = 4 makes the common "flip between two or three tabs" case free; the fifth-oldest tab pays one Snapshot on return. A hidden `<terminal-grid>` must not paint, must not hold focus, and (per 04) may ask the server to pause deltas (`SetAttachMode{passive}`) so hidden replicas cost only memory. N is a constant in `SurfaceHost.tsx`, tunable later from config.

Focus: when the active tab changes, `<SurfaceHost>` calls `focus()` on the active grid's ref after commit. Tab-key focus traversal stays in Rust (gpuix limit), which is fine: Tab must reach the shell.

**`<CommandPalette>`** is a single `<anchored>` positioned at top-center, 560 px wide, containing `<input autoFocus onChange onKeyDown>` and a plain `<div>` list (not `virtual-list`: at most a few dozen rows; and nested scrolling is unsupported, so the list is capped at 8 visible rows and scrolls by shifting the window of items, not by an inner scroll container). Modes: `commands` (⌘/Ctrl+Shift+P) and `sessions` (⌘/Ctrl+K). Fuzzy match is a ~30-line subsequence scorer; no dependency.

**`<Banner>`** shows one message at a time with an action button: `disconnected` → **Reconnect**; `mismatch` → **Restart server** (the text states running processes will be killed, Q31); `reconnecting` → spinner only, no action.

---

## 5. Command registry & keybindings (Q29)

```ts
export interface Command {
  id: string;                       // 'tab.new'
  title: string;                    // 'New Tab'
  shortcut: Keybinding[];           // platform-resolved at registry build time
  when?: (s: WorkspaceState) => boolean;   // enablement, also hides from palette
  run: (ctx: CommandContext) => void | Promise<void>;   // ctx = { store, client, native, platform }
}
export type Keybinding = { mods: Array<'mod'|'shift'|'alt'|'ctrl'>; key: string }; // 'mod' = ⌘ | Ctrl
```

The modifier story (Q4: no prefix key, ordinary app shortcuts): `mod` is ⌘ on macOS. On Linux, plain Ctrl+letter *is* a terminal keystroke (Ctrl+T = transpose in readline, Ctrl+K = kill line), so `mod` maps to **Ctrl+Shift** for every binding in the default table below; the table is written once with `mod` and resolved by `platform/keys.ts`. Users override per command in `config.keybindings`.

| Command id | Title | macOS | Linux | `when` |
|---|---|---|---|---|
| `tab.new` | New Tab | ⌘T | Ctrl+Shift+T | connected |
| `tab.close` | Close Tab | ⌘W | Ctrl+Shift+W | active tab |
| `tab.next` | Next Tab | ⌘⇧] , Ctrl+Tab | Ctrl+Shift+] , Ctrl+Tab | >1 tab |
| `tab.prev` | Previous Tab | ⌘⇧[ , Ctrl+Shift+Tab | Ctrl+Shift+[ , Ctrl+Shift+Tab | >1 tab |
| `tab.goto(1–9)` | Go to Tab N | ⌘1…⌘9 | Alt+1…Alt+9 | — |
| `session.new` | New Session | ⌘N | Ctrl+Shift+N | connected |
| `session.switch` | Switch Session… | ⌘K | Ctrl+Shift+K | connected |
| `session.rename` | Rename Session | ⌘R | Ctrl+Shift+R | active session |
| `view.toggleVerticalTabs` | Toggle Vertical Tabs | ⌘⇧B | Ctrl+Shift+B | — |
| `edit.copy` | Copy | ⌘C | Ctrl+Shift+C | selection present |
| `edit.paste` | Paste | ⌘V | Ctrl+Shift+V | active surface |
| `surface.clearScrollback` | Clear Scrollback | ⌘⇧K | Ctrl+Shift+L | active surface |
| `app.reconnect` | Reconnect | — | — | — |
| `palette.commands` | Command Palette | ⌘⇧P | Ctrl+Shift+P | — |
| `app.quit` | Quit | ⌘Q | Ctrl+Shift+Q | — |

`Ctrl+Tab` on macOS and `Alt+digit` on Linux are the only non-`mod` entries and are listed explicitly. `tab.goto` is one command with a numeric argument to keep the palette clean.

**Feeding `passthroughShortcuts`.** The native `<terminal-grid>` owns keyboard input (Q23) and would otherwise encode ⌘T as bytes or eat it. `<SurfaceHost>` passes `passthroughShortcuts={registry.passthroughList()}` — the flattened, platform-resolved bindings serialised as gpuix keystroke strings (`"cmd-t"`, `"ctrl-shift-t"`, `"ctrl-tab"`). The element declines exactly these; GPUI bubbles them to the app root, where `<App onKeyDown={dispatchKey}>` normalises the event to a `Keybinding` and runs the first enabled matching command. `Ctrl+Shift+C/V` on Linux are also *conventional* terminal copy/paste, so the terminal loses nothing. The same list is what the palette displays as hints, so there is one source of truth: the registry.

Edge: while the palette is open, `dispatchKey` first routes Esc/↑/↓/Enter to the palette; commands still fire (⌘K toggles session mode from within the palette).

---

## 6. Tab & session behaviours (Q20–Q22)

- **New tab** → `CreateTab{sessionId, cwd: activeSurface.cwd}`. The cwd comes from the server's surface record (it tracks the foreground process cwd via `/proc` or `proc_pidinfo`, 03), never from the client. If the active surface has exited, its last known cwd is used.
- **Close tab** → if `surface.hasForegroundChild` (server-reported: a process other than the shell is in the foreground group) show an inline confirm in the tab ("Close? running: `vim`" with Enter/Esc), else `CloseTab` immediately. Closing kills the surface (Q21). Last tab of the last session: server re-seeds; client just renders the echo.
- **Exited surfaces (Q22)** → tab shows a dim `⏻ 0`/`⏻ 130` badge; the grid stays readable. Enter while the exited grid is focused, or clicking the badge, sends `CloseTab`. The native element forwards Enter to React only when `status === 'exited'` (04: `onExited` flips a prop `inputEnabled=false`; the element then bubbles all keys).
- **Session switcher** → palette in `sessions` mode lists sessions (name, tab count, ✓ active) plus a final row **New Session "‹query›"** when the query matches no existing name — mirrors the demo. Enter on a session → `ActivateSession`; on the new-row → `CreateSession{name}` then activate. Switching swaps the strip; per-session active tab is remembered (`activeTabBySession`, server-owned).
- **Rename** → `session.rename` turns the `<SessionChip>` into an `<input>` in place (`ui.renamingSessionId`); Enter → `RenameSession`, Esc → cancel. Empty names rejected client-side.
- **Quit** → closes the window; the client sends `Detach` (best effort) and exits. Nothing is killed (Q21).

---

## 7. Visual design tokens

Derived from `blurred-window.tsx`; one `theme/tokens.ts` module, consumed by every chrome component (never inline hex).

| Token | Value | Use |
|---|---|---|
| `bg.glass` | `#FFFFFF0D` | tab strip, palette, banner panels |
| `bg.glassHover` | `#FFFFFF1A` | hovered tab / list row |
| `bg.glassActive` | `#FFFFFF26` | active tab |
| `border.glass` | `#FFFFFF1F`, width 1 | panel borders |
| `fg.primary` | `#F2F2F2` | tab titles, palette text |
| `fg.muted` | `#FFFFFF80` | session chip, shortcuts, exited badge |
| `fg.danger` | `#FF6B6B` | banner errors, close confirm |
| `accent` | `#7AA2F7` | focus ring, selected palette row bar |
| `radius.panel / tab / chip` | 16 / 8 / 999 | |
| `font.chrome` | system UI, 12.5 px; chip 11.5 px; palette input 14 px | |
| `padding.trafficLights` | top 58 (macOS only) | `<TitleBarSpacer>` |

Tab strip metrics: horizontal strip height 36, tabs 28 high, max width 220, gap 4, strip padding `0 12`; vertical strip width 220, tabs full width, 2-line title clamp. Every `<text>` sets `color` explicitly (GPUI does not inherit). Hover states use gpuix's `hover` style prop (`hover: { backgroundColor: tokens.bg.glassHover }`); focus ring is a 1 px `accent` border on the focused palette row/input (drawn, not outline). Density: a single "comfortable" density in v1.

**Opaque fallback (Linux non-blurred / `'opaque'`).** The alpha-white glass tokens look grey-on-black on an opaque window; `tokens.ts` exports `glassTokens` and `opaqueTokens` (`bg.glass → #1E1E22`, `bg.glassActive → #2A2A30`, border `#2E2E36`) and `<App>` picks by the resolved `windowBackground`. The terminal palette itself is a `theme` prop on `<terminal-grid>` (04) and is independent of chrome tokens.

---

## 8. Dev workflow

```
bun install                       # workspaces: packages/app, packages/protocol-ts, packages/native
just native                       # cargo build -p st-native → packages/native/superterminal-native.<triple>.node
just server-dev                   # cargo run -p st-server -- --foreground   (second terminal, logs to stderr)
SUPERTERMINAL_SOCKET=/tmp/st-dev.sock DEBUG='st:*' bun --hot packages/app/src/app.tsx --no-spawn
```

`--hot` re-evaluates modules while keeping the gpuix window (the `render()` root is created once in a module guarded by `globalThis.__stRoot`; on hot reload we call `root.render(<App/>)` again). The store lives on `globalThis` in dev so hot reloads keep state. Logging: a 20-line `debug(ns)` helper honouring `DEBUG=st:control,st:store,...` and writing to stderr; no dependency.

**How `@gpuix/react` finds our native module — decision: `NAPI_RS_NATIVE_LIBRARY_PATH`.** Verified from `packages/native/index.js` in gpuix 0.6.0: the napi-rs loader checks `process.env.NAPI_RS_NATIVE_LIBRARY_PATH` first and `require`s that file directly, before trying `./gpuix-native.<triple>.node` or the `@gpuix/native-<triple>` optional packages. Our `st-native` crate depends on `gpuix-native` as an rlib and re-exports its napi classes (`GpuixRenderer`, `TestGpuixRenderer`) alongside our `TerminalGridFactory` registration, so the resulting `.node` is a drop-in superset. `packages/app/src/native/locate.ts` computes the path (dev: `packages/native/superterminal-native.<triple>.node`; compiled: see §10) and sets the env var; `bunfig.toml` `[run] preload = ["./packages/app/src/native/preload.ts"]` guarantees it runs before `@gpuix/react` is evaluated (ESM imports hoist, so setting it in `app.tsx` would be too late). Rejected alternatives: root `package.json` `overrides: { "@gpuix/native": "npm:@superterminal/native@…" }` — Bun's overrides doc lists `npm:` and `catalog:` values but not `workspace:`, and would require our package to ship a compatible `index.js`; `bunfig.toml [install.overrides]` — not documented in Bun 1.4.0. The env-var route needs no package-manager tricks and works identically under `bun test`.

---

## 9. Testing (Q33 item 3)

- **Reducers & registry** (`bun test`, pure): `applyServerEvent` against recorded event fixtures; `applyUiAction`; keybinding resolution per platform; palette fuzzy scorer; `resolveLinuxBackground` with fake env.
- **Control client**: `test/fakeServer.ts` — a `Bun.listen({ unix })` server speaking the same NDJSON with scripted replies and pushed events. Tests cover correlation, timeouts, out-of-order responses, reconnect after `close()`, Hello version refusal. Also reused by integration tests of the whole store (`client → store → selectors`).
- **Components**: gpuix `createTestRoot()` from `@gpuix/react/testing` (requires the native module built with test-support; `hasNativeTestRenderer` gates the suite so `bun test` still passes on machines without it). `TabStrip` (active/exited/confirm states), `CommandPalette` (filtering, Enter/Esc, session mode "New Session" row), `Banner`. Queries via `findByTestId`/`findByText`; interactions via `nativeSimulateClick`/`nativeSimulateKeystrokes`.
- **Snapshots**: `toJSON()` of the tree for each `TabStrip` variant, stored under `__snapshots__`; reviewed like code.
- `<terminal-grid>` itself is tested in Rust (04/06); in TS tests it renders as an inert custom element.

---

## 10. Packaging (Q35)

```
just release-client:
  cargo build --release -p st-native  (per target)
  bun build --compile packages/app/src/app.tsx \
    --asset packages/native/superterminal-native.<triple>.node \
    --outfile dist/superterminal
```

Verified (Bun 1.4.0 docs + `bun build --help`): `--asset` embeds a file or directory "preserving its relative path"; embedded files appear under a virtual `/$bunfs/…` tree that `node:fs` treats as real (`existsSync`, `statSync`, `readdirSync`, `readFileSync`), `Bun.embeddedFiles` enumerates them as `Blob`s with `name`, and `import x from './f' with { type: 'file' }` yields the `/$bunfs/root/…` path. The docs also state `require('./addon.node')` embeds a native addon directly. What the docs do **not** state is whether `dlopen` works from a `/$bunfs/` path when the addon is loaded indirectly (our case: gpuix's loader `require`s a path from an env var). Plan: `native/locate.ts` detects the compiled case (`process.execPath` is not `bun`), and if the located asset path starts with `/$bunfs/`, copies it once to `$XDG_CACHE_HOME/superterminal/<build_id>/superterminal-native.node` (skip if present) and points `NAPI_RS_NATIVE_LIBRARY_PATH` there — guaranteed to dlopen. If M0 shows direct `/$bunfs/` loading works, the copy step is deleted.

`superterminald` is a plain `cargo build --release` binary copied into `dist/` beside the client; `ensureServer` finds it via `dirname(process.execPath)` (§1). `superterminal --version` prints `superterminal <semver> (<git sha>, proto <PROTO_VERSION>)` from a generated `build-info.ts` (written by `just` before the build; the same `build_id` goes into `Hello`). macOS `.app`, signing, notarisation: M6.

---

## 11. Accessibility & i18n (deferred; do not break)

- Every interactive chrome element has a `testId` and a text label; no icon-only controls without a title (the close ✕ and badge have `title`/tooltip via gpuix headless tooltip).
- All commands are reachable from the palette by name, so nothing is mouse-only.
- Contrast: `fg.muted` on glass is ≥ 4.5:1 against a dark wallpaper; opaque tokens are checked the same way.
- Strings live in `i18n/strings.ts` as a flat `en` map from day one (cheap; makes later locales a data change). No pluralisation library.
- Reduced motion: `motion.div` use is limited to palette fade and toast slide; both read a `reducedMotion` flag (config now, OS later).
- Screen-reader exposure of the grid depends on GPUI's accessibility tree; nothing to do in the app until GPUI exposes it.

---

## 12. Module layout

```
packages/app/
  package.json            name @superterminal/app; deps: @gpuix/react, react, zod, @superterminal/protocol-ts
  tsconfig.json           "jsxImportSource": "@gpuix/react", strict
  bunfig.toml             [run] preload = ["./src/native/preload.ts"]; [test] preload same
  src/app.tsx             argv, config, ensureServer, windowOptions, render(<App/>)
  src/App.tsx             providers, root onKeyDown, layout of §4
  src/config/schema.ts    zod Config + defaults
  src/config/load.ts      path resolution, Bun.TOML.parse, warnings
  src/platform/detect.ts  os, wsl, wayland, isCompiled
  src/platform/windowOptions.ts  render() options, resolveLinuxBackground
  src/platform/keys.ts    Keybinding parse/format, 'mod' resolution, gpuix keystroke strings
  src/control/client.ts   Bun.connect, NDJSON framing, correlation, reconnect
  src/control/ensureServer.ts    probe, spawn, retry
  src/store/store.ts      useSyncExternalStore store, useWorkspace(selector)
  src/store/reducers.ts   applyServerEvent, applyUiAction
  src/store/selectors.ts
  src/commands/registry.ts  Command type, buildRegistry(platform, config)
  src/commands/defaults.ts  v1 command list (table §5)
  src/components/{TitleBarSpacer,TabStrip,Tab,SessionChip,NewTabButton,SurfaceHost,
                  CommandPalette,Banner,StatusToasts}.tsx
  src/theme/tokens.ts     glassTokens, opaqueTokens
  src/native/locate.ts    find .node (dev / $bunfs copy), set NAPI_RS_NATIVE_LIBRARY_PATH
  src/native/preload.ts   runs locate() before any import of @gpuix/react
  src/native/terminalGrid.ts  TS types for <terminal-grid> props/events (mirrors 04)
  src/util/debug.ts       DEBUG=st:* logger
  src/i18n/strings.ts
  test/fakeServer.ts, test/**/*.test.ts(x), test/__snapshots__/
packages/protocol-ts/
  package.json            name @superterminal/protocol-ts (types only)
  src/generated/*.ts      ts-rs output from st-proto (committed, CI-checked)
  src/index.ts            re-exports + RequestType/ReqParams/ResOk helpers
  src/version.ts          PROTO_VERSION (generated)
packages/native/
  package.json            name @superterminal/native; holds built .node files (gitignored)
```

---

## Open questions

1. **Does `dlopen` work from `/$bunfs/`?** Bun docs confirm `node:fs` reads and direct `require('./addon.node')` embedding, but not env-var-path `require` of an embedded `.node`. Decide in M0; the cache-copy fallback in §10 stands until then.
2. **`detached` in `Bun.spawn`.** Q30 says "detached, unref". `proc.unref()` is documented; the `detached: true` option (new process group so the daemon survives the terminal that launched the client) should be confirmed on 1.4.0 during M0, else the server double-forks itself.
3. **Socket topology for the data plane.** §1 assumes the native module gets the *same* socket path and the `Hello` selects plane, or a sibling `data.sock`. `02/03` must pick; the app just forwards `socketPath` to the native module.
4. **Passive attach for hidden tabs** (`SetAttachMode{passive}` in §4) is a request to 02/04; without it, N = 4 still works but hidden replicas receive deltas.
5. **Server-reported `hasForegroundChild` and `cwd`** (§6) need to be in `SurfaceStatus` events in 02; the tab-close confirm cannot work otherwise.
6. **`display: 'none'` semantics for custom elements** in gpuix: does a hidden custom element keep its GPUI focus handle and get layout? 04 must specify that the element does nothing when not laid out.
7. **Enter-closes-exited-tab routing**: whether the native element bubbles *all* keys once exited (assumed here) or React passes `inputEnabled=false`; either way 04 owns it.
8. **Hot reload and the native root**: `bun --hot` re-running the entry must not call `render()` twice; the `globalThis.__stRoot` guard is assumed to work with `@gpuix/react`'s root API — verify in M0.
9. **Linux `mod` = Ctrl+Shift** conflicts with shells that bind Ctrl+Shift+letter (rare; kitty-protocol users). Acceptable for v1; overrides exist.
10. **Hello ordering vs. snapshot size**: whether the server includes the full `WorkspaceSnapshot` in the Hello reply (assumed) or the client must request it. 02 decides.
