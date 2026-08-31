# 06 — Testing, performance, CI

> **Addendum (00-grilling §F):** Q47 — render/e2e tests run on macOS CI, Linux headless is a non-blocking experiment, the M2 gate is decided by macOS numbers with WSL2 within 2×, nightly perf runs on a self-hosted runner. Milestone names M0–M6 in `07-milestones.md` are adopted as proposed here.

Status: plan only. Implements Q33 (testing strategy), Q27 (frame budget), Q36c (shaping-cache gate), Q3 (platform matrix). Nothing here re-decides a frozen answer; conflicts are collected under *Open questions*.

Vocabulary is from CONTEXT.md: Surface, Replica, Snapshot, Delta, Attach, Workspace, Session, Tab.

---

## 1. Test pyramid

Rule of thumb: every crate has fast unit tests runnable with `cargo test -p <crate>` in under 30 s without a GPU or a display; integration tests that need a PTY live in `tests/` and are gated behind `#[cfg(unix)]`; anything that needs a window is in a separate `e2e` job and never blocks a PR by itself until M4.

### 1.1 `st-proto` (frames + postcard payloads)

Pure data, no I/O. `proptest` is a dev-dependency here and in `st-client-core`.

| Test | Asserts |
|---|---|
| `frame_header_roundtrip` | `u32 len | u16 type | payload` encodes/decodes for all `FrameType` variants (exhaustive via `strum::IntoEnumIterator`). |
| `prop_payload_roundtrip_<Msg>` | proptest: for every message struct (`Hello`, `Attach`, `Snapshot`, `Delta`, `Input`, `FetchHistory`, `Resize`, `SetSelection`, `HistoryPage`), `decode(encode(m)) == m`. Uses `Arbitrary` derives. |
| `prop_cell_pack_unpack` | packed `u32 codepoint | u16 style_idx | u8 flags` round-trips; codepoints > U+10FFFF and surrogates are rejected, not wrapped. |
| `max_frame_len_enforced` | a header declaring `len > MAX_FRAME` (proposed 16 MiB) yields `Err(FrameTooLarge)` *before* any allocation. |
| `truncated_frame_is_incomplete_not_error` | a decoder fed a partial frame returns `NeedMore(n)`; the same bytes plus the tail decode cleanly (streaming decoder correctness). |
| `unknown_frame_type_is_skippable` | an unknown `u16 type` with a valid length is reported as `Unknown{ty, len}` and the stream stays in sync (forward compatibility). |
| `hello_same_major_accepts` / `hello_lower_major_rejects` / `hello_higher_minor_accepts` | version negotiation per Q31: only a lower *major* is refused; the refusal message carries `server_version` and `min_client_version`. |
| `delta_seq_is_monotonic_in_fixture` | recorded delta stream fixture has strictly increasing `seq`; the decoder exposes gaps as `SeqGap`. |
| `style_table_entry_roundtrip` | `Style{fg,bg,attrs}` in RGB, indexed, and default color variants. |
| `json_control_messages_roundtrip` | serde_json round trip of every control-plane message; snapshot test (`insta`) of the JSON shape so `packages/protocol-ts` can be checked against the same fixtures. |
| `ts_fixture_parity` | `st-proto` writes `fixtures/control/*.json` in a build step; `packages/protocol-ts` tests parse the identical files (shared golden files, section 1.6). |

### 1.2 `st-core` (server terminal engine: alacritty_terminal + PTY + damage → Delta)

Integration tests drive a real PTY with `portable-pty`, run `bash -c 'printf …'` (or `sh -c` where bash is absent), wait for exit, and compare against an expected `Snapshot`. Fixtures are plain files under `crates/st-core/tests/fixtures/*.ansi`. All tests use a helper `run_fixture(cols, rows, script) -> (Surface, Vec<Delta>, Snapshot)`.

| Test | Asserts |
|---|---|
| `pty_echo_hello_snapshot` | `printf 'hello\n'` → row 0 == `hello`, cursor at (1,0), no styles beyond default. |
| `sgr_fixture_styles_interned` | 16-color, 256-color, truecolor, bold/italic/underline/strikethrough all present; each distinct style gets exactly one `style_idx` (`HashSet` size == distinct styles). |
| `style_interning_is_deterministic` | running the same fixture twice produces byte-identical style tables and `Snapshot`s (no `HashMap` iteration order leaks). |
| `delta_replay_equals_snapshot` | fold every emitted `Delta` onto the first `Snapshot` (using `st-client-core::Replica`) and compare with the final `Snapshot`. This is the cornerstone test; the proptest version is in 1.3. |
| `damage_dirty_rows_are_minimal` | printing one char on row 5 emits a `Delta` whose `rows` set is exactly `{5}` (plus cursor). |
| `resize_shrink_then_grow` | 80×24 → 40×24 → 80×24 with a 100-line fixture: reflow matches alacritty's own `Term::resize` result; the client receives a full `Snapshot` after resize, not a `Delta`. |
| `alt_screen_enter_exit` | `tput smcup; printf X; tput rmcup` → mode flag toggles in two `Delta`s; primary grid restored; `scrollback_appended == 0` while in alt screen. |
| `scrollback_ids_monotonic` | `seq 1 5000` on 24 rows → `scrollback_appended` sums to 4976; `FetchHistory{from: 0, count: 1000}` returns lines `1..=1000`; ids never reused after `Clear Scrollback`. |
| `osc7_cwd_tracking` | `printf '\e]7;file://host/tmp/x\a'` → `Surface.cwd == "/tmp/x"`; percent-decoding and wrong-hostname handling. |
| `mouse_mode_flags` | `\e[?1000h`, `\e[?1002h`, `\e[?1006h` set `mode.mouse` and `mode.sgr_mouse` correctly; `\e[?2004h` sets bracketed paste. |
| `process_exit_sets_exited` | `bash -c 'exit 3'` → `Surface.state == Exited{code: 3}` (Q22), grid still readable. |
| `sighup_on_destroy` | destroying a Surface running `sleep 1000` terminates the process group within 1 s. |
| `wide_and_zero_width_cells` | CJK + combining marks: wide char occupies two cells with the spacer flag; ZWJ sequences stay in one cell. |

### 1.3 `st-client-core` (Replica, encoders, selection, history paging)

| Test | Asserts |
|---|---|
| `prop_replica_equals_server_grid` | **The property test from Q33.** proptest generates random byte streams biased toward ANSI (a grammar producing CSI/OSC/SGR/printable/newline/resize events). Feed them to alacritty on the "server" side producing `Snapshot` + `Delta`s; apply the deltas to a `Replica`. Assert `Replica::to_grid() == server grid` after every step. 1 000 cases in CI, 100 000 nightly. |
| `replica_rejects_seq_gap` | applying `seq 7` after `seq 5` returns `NeedResync`, which the native module turns into a re-`Attach`. |
| `history_paging_fetch_windows` | scrolling to offset 2 500 with a 1 000-row page size requests pages 2 and 3 exactly once; pages are evicted LRU beyond 10 pages. |
| `key_encoder_xterm_table` | table-driven against xterm reference sequences: arrows (normal/application cursor mode), Home/End, F1–F12, Ctrl+letters, Alt+letter (ESC prefix), Backspace (`0x7f`), Shift+Tab (`\e[Z`), keypad in DECKPAM. Table is a CSV so it can be diffed against `xterm -xrm '*modifyOtherKeys: 0'` output. |
| `key_encoder_declines_app_shortcuts` | Ctrl/⌘+T, K, W, 1–9 return `Declined` so GPUI dispatches them to React (Q23). |
| `mouse_encoder_x10_and_sgr` | press/release/drag/wheel at (col 200, row 3) in X10 (clamped at 223) and SGR (`\e[<0;201;4M`) modes. |
| `mouse_shift_overrides_to_selection` | Shift+drag while `mode.mouse != none` produces a selection, not reporting (Q24). |
| `selection_geometry_linear_word_block` | anchor/head normalization, word boundaries on punctuation, block selection rectangles, selection across wrapped rows yields one logical line without a `\n`. |
| `selection_survives_scrollback_append` | selection anchored in history stays on the same text after 500 new rows. |
| `selection_to_text_trailing_whitespace_trimmed` | copy of a selected row excludes padding cells unless the row has the `wrapped` flag. |

### 1.4 `st-server` (daemon: actors, persistence, lifecycle, backpressure)

Tests run the server in-process on a temp `$XDG_RUNTIME_DIR` with a fake clock (`tokio::time::pause`).

| Test | Asserts |
|---|---|
| `workspace_actor_create_session_tab_surface` | control command sequence creates Session → Tab → Surface; the echoed `WorkspaceChanged` matches the internal document. |
| `close_last_tab_reseeds_session` | Q21: closing the last Tab of the last Session yields one fresh Tab. |
| `close_tab_kills_surface` | Surface process gone after `CloseTab`. |
| `view_state_persists_across_detach` | `SetSelection` + `SetScroll` then detach/attach → same `view_state`. |
| `persistence_roundtrip` | Workspace document → `workspace.json` → reload → equal modulo Surface PIDs; cwd per Surface restored (Q18). |
| `persistence_write_is_atomic` | write to temp + rename; a simulated crash mid-write leaves the previous file intact. |
| `idle_shutdown_only_when_no_surfaces` | with N=5 min idle, zero Surfaces → exits; one Surface alive → never exits (Q30). |
| `single_instance_lock` | second server on the same runtime dir exits with code 2 and a clear message; a stale lock with a dead PID is reclaimed. |
| `hello_version_mismatch_banner_payload` | lower major → refusal frame as in 1.1 and the control-plane `Error` JSON the app renders as the banner (Q31). |
| `slow_client_coalesces_deltas` | a fake data-plane client that reads 1 frame/100 ms while `yes` runs: server memory stays bounded (< 4 MiB per client queue), the client always ends with the final grid, and dirty rows are merged not duplicated. |
| `throttle_120hz_per_client` | with 10 000 grid changes/s, one client receives ≤ 125 Deltas/s; a second fast client is not slowed by the slow one. |
| `attach_sends_snapshot_then_deltas` | `Attach` → exactly one `Snapshot` followed by `Delta`s whose first `seq` is `snapshot.seq + 1`. |
| `two_clients_one_surface` | input from client A is visible to client B; both replicas equal the server grid. |

### 1.5 `st-native` (gpuix custom element)

Honest feasibility: GPUI can be exercised headlessly with its `TestAppContext` / `test` feature (Zed's own crates do this), but gpuix's `TestGpuixRenderer` is documented as **GPU-backed** (Metal on macOS, DirectX on Windows, "Linux: not yet"). So on Linux CI we get **no** gpuix-level renderer tests; we keep the element logic GPU-free and test it directly.

GPU-less (the bulk):

| Test | Asserts |
|---|---|
| `run_grouping_same_style_merges` | a row `[a:s1 b:s1 c:s2]` yields runs `[("ab",s1),("c",s2)]`. |
| `run_grouping_splits_at_wide_and_cursor` | wide cells and the cursor cell terminate runs (cursor gets its own run for inversion). |
| `run_cache_hit_on_identical_row` | two consecutive frames with an unchanged row do zero shaping calls (counter on a `ShapeFn` trait object). |
| `run_cache_invalidates_on_style_table_change` | changing `style_idx 3`'s color invalidates only runs referencing 3. |
| `run_cache_bounded` | LRU capacity (proposed 64 k runs) is respected under random rows. |
| `viewport_rows_for_scroll_offset` | which Replica/history rows are visible for a given scroll offset and row height. |
| `hit_test_pixel_to_cell` | pixel → (col,row) including fractional cell widths and the scrollbar gutter. |

Headless GPUI (feasible, macOS + Linux, uses `gpui::TestAppContext`, no window server needed): `element_paints_expected_quads` (background quads per style run) and `element_requests_repaint_only_on_delta` (Q27 repaint policy via `cx.notify()` counter). Both are behind `--features gpui-test` because they require the vendored Zed build; they run in the `native` CI job, not in `cargo test` by default. If `TestAppContext` proves unusable outside the Zed workspace, these two drop to the e2e layer (open question 3).

### 1.6 `packages/app` and `packages/protocol-ts` (bun test)

Component tests use `createTestRoot()` from `@gpuix/react/testing` (the `TestGpuixRenderer`). Because that renderer is GPU-backed, these run on the macOS runner; on Linux the reducer and client tests still run because they do not need the renderer.

| Test | Asserts |
|---|---|
| `TabStrip renders one tab per Tab and marks active` | `renderer.getPaintedText()` includes titles; active tab has the `active` testId. |
| `TabStrip traffic-light padding on darwin only` | left padding prop equals `trafficLightX + width` on `process.platform === 'darwin'`, 0 elsewhere (Q28). |
| `Palette filters commands and runs on Enter` | `simulateKeystrokes('k' with ctrl)`, type `new t`, Enter → `run()` of `New Tab` called once. |
| `Palette closes on Escape and restores focus to terminal-grid` | focus event drained via `drainEvents`. |
| `ExitedBadge shows for Exited surfaces` | Q22 badge visible; Enter closes tab. |
| `VersionBanner shows on protocol error with Restart action` | Q31. |
| `WorkspaceStore reducer: applyWorkspaceChanged is idempotent` | applying the same echo twice equals applying once. |
| `WorkspaceStore reducer: optimistic create then echo reconciles ids` | temp id replaced, order preserved. |
| `WorkspaceStore reducer: closing active tab picks neighbor` | matches spec in 05-client-app.md. |
| `ControlClient reconnects with 3 s backoff` | fake TS server (`Bun.listen` on a temp Unix socket) dropped → reconnect after 3 s using `Bun.sleep` with a mocked timer. |
| `ControlClient spawns daemon when socket absent` | `Bun.spawn` mocked; asserts detached + `unref` (Q30). |
| `ControlClient message framing: newline-delimited, partial reads` | two messages split across three chunks decode to two objects. |
| `protocol-ts parses all st-proto golden fixtures` | every `fixtures/control/*.json` validates against the zod schemas (parity with 1.1). |

### 1.7 End-to-end

**Feasibility.** Three ways to get a real client on Linux CI: (a) Xvfb + Vulkan via `lavapipe` (Mesa software Vulkan) — GPUI on Linux needs Vulkan, Xvfb alone is not enough; Zed's CI does not run GUI e2e on Linux, which is a warning sign. (b) `sway --unsupported-gpu` headless with `WLR_BACKENDS=headless` + lavapipe — same Vulkan requirement, more setup. (c) macOS runner with `GPUIX_BACKGROUND=1` and gpuix's `launch()` automation API — documented and expected to work. **Decision for this plan: e2e runs on `macos-14` first; the Linux Xvfb+lavapipe variant is an M3 experiment that is allowed to be `continue-on-error` until it has passed 20 consecutive nightly runs.**

Minimal e2e (`packages/app/e2e/smoke.test.tsx`), one scenario, ~30 s:

```ts
const daemon = Bun.spawn(['superterminald', '--foreground', '--runtime-dir', tmp])
const app = await launch({ command: 'bun', args: ['src/main.tsx'],
  env: { GPUIX_BACKGROUND: '1', SUPERTERMINAL_RUNTIME_DIR: tmp } })
await app.getByTestId('new-tab').click()
const grid = app.getByType('terminal-grid')
await grid.waitFor()
await grid.press('printf "e2e-ok\\n"\n')
await waitUntil(async () =>
  (await app.getCustomProp(grid, 'contentLines')).some(l => l.startsWith('e2e-ok')))
await app.close(); daemon.kill()
```

`contentLines` is a debug-only custom prop the element exposes (rows of the visible Replica as strings). Two more e2e scenarios join at M4: *detach/reattach keeps output and selection* (kill client, relaunch, assert `contentLines` and selection rect) and *server auto-spawn*.

---

## 2. VT conformance

**alacritty ref recordings as fixtures.** `alacritty_terminal/tests/ref/<name>/` contains `alacritty.recording` (raw PTY bytes), `size.json`, `grid.json` (expected `Grid<Cell>`), `config.json`. 45 recordings today, including `vttest_*`, `tmux_htop`, `vim_large_window_scroll`, `zsh_tab_completion`, `history`, `wrapline_alt_toggle`, `zerowidth`. We vendor the directory (Apache-2.0, attribution kept) into `crates/st-core/tests/alacritty-ref/` pinned to the alacritty_terminal version in `Cargo.lock`, and run each recording twice:

1. **Engine conformance** — feed the recording to our `Surface` (which wraps alacritty) and compare the resulting `Snapshot` to `grid.json`. This mostly proves our Snapshot extraction is lossless, since the engine *is* alacritty.
2. **Replica conformance** — feed the recording in random-sized chunks (seeded) so Deltas are emitted at arbitrary boundaries, apply them to a `Replica`, compare to `grid.json`. This is the test that catches delta bugs on realistic streams (`tmux_htop` exercises alt screen + heavy SGR; `history` exercises scrollback ids).

Comparison ignores fields our Replica does not model (alacritty's per-cell `extra` hyperlinks until M5). A macro generates one `#[test]` per directory so failures are named.

**vttest manual checklist (M3 gate).** Run `vttest` in a tab and record pass/fail for menu 1 (cursor movements), 2 (screen features: wrap, origin mode, tabs, DECALN), 3 (character sets — only the ASCII/DEC line drawing rows), 6.1–6.3 (SGR, DECRQSS ignored), and 11.6 (mouse: X10, normal, SGR). Stored as `docs/qa/vttest-M3.md` with screenshots from `renderer.captureScreenshot`. Not automated.

**esctest** (Apple/iTerm2's `esctest`): practical only against a program that answers queries on a real terminal. We add a `just esctest` recipe that runs it against `superterminal` via its own `--expected-terminal xterm` profile, non-blocking, results checked in as a text report at M5. If it needs more than one day to wire, it is dropped.

---

## 3. Performance harness and budgets

### 3.1 Scenarios (`crates/st-bench`, binary `st-bench`)

`st-bench` replays a **recorded PTY byte stream** deterministically into a Surface (server-only mode) or into server + native client (full mode), with a fake clock so the 120 Hz throttle and coalescing behave identically run to run. Recordings are produced with `SUPERTERMINAL_RECORD=1` (section 6) and stored under `bench/recordings/*.rec` (Git LFS if > 5 MiB).

| Scenario | Source | Duration |
|---|---|---|
| `cat-100mb` | `cat` of a generated 100 MB text file (mixed line lengths, some SGR) | until EOF |
| `yes-10m` | `yes | head -n 10000000` | until EOF |
| `btop-1s` | 60 s recording of `btop` (fallback `htop`) at 1 s refresh, 200×50 | 60 s |
| `nvim-scroll` | `nvim` holding Ctrl-D through a 20 k-line file, 120×40 | 20 s |
| `find-10tabs` | 10 Surfaces each replaying a `find /` recording concurrently, one visible | 30 s |
| `attach-cold` | server with 5 k-line scrollback; measure `Attach` → first painted frame | 20 reps |
| `echo-probe` | input→paint latency: client sends `Input(b"x")`, server-side shell echoes, measure until the frame containing `x` is painted | 200 reps |

### 3.2 Metrics

Collected as `tracing` spans and events, exported by `tracing-chrome` (Perfetto/Chrome trace) and summarized to JSON by `st-bench --summary`:

- `frame_ms` p50 / p99 (from GPUI's frame callback; cross-checked with `renderer.getDebugFrameOverlayStats()` `p99Ms` in the gpuix `debugFrameOverlay`).
- `input_to_paint_ms` p50 / p99 (echo probe).
- `attach_to_first_paint_ms` (median of 20).
- `deltas_per_sec` sent per client, `coalesce_ratio` = grid changes / Deltas sent.
- `rows_shaped_per_frame` and `shape_cache_hit_rate` (st-native counters).
- `rss_mb` server and client (from `/proc/self/status` on Linux, `task_info` on macOS) sampled at 1 Hz.

### 3.3 Pass/fail thresholds (from Q27)

| Metric | Threshold | Scenario |
|---|---|---|
| `frame_ms` p99 | ≤ 16.6 ms (60 fps) | `cat-100mb`, `yes-10m`, `btop-1s`, `nvim-scroll` |
| `frame_ms` p50 | ≤ 4 ms | all |
| `input_to_paint_ms` p99 | ≤ 16.6 ms (< 1 frame) | `echo-probe` on an idle shell |
| `input_to_paint_ms` p99 | ≤ 33 ms | `echo-probe` while `find-10tabs` runs |
| `attach_to_first_paint_ms` | < 100 ms warm | `attach-cold` |
| `deltas_per_sec` per client | ≤ 125 | `yes-10m` |
| final grid | equals server grid | every scenario (correctness is a perf gate too) |
| `rss_mb` client | ≤ 250 MB after `find-10tabs` | |
| `rss_mb` server | ≤ 60 MB + 4 MB per Surface with 10 k scrollback | `find-10tabs` |

Thresholds apply to the reference machines (dev WSL2 box, `macos-14` runner). CI treats a regression as **> 15 % worse than the 7-day nightly median** rather than an absolute number, because runner variance is large; absolute numbers are the release gate at M5 on the dev machines.

### 3.4 M2 shaping-cache gate (Q36c) — the exact experiment

Question: is per-run shaping through GPUI's `shape_line` with a `(text, style)` cache fast enough for a full-screen redraw at 60 fps?

Setup: `st-bench --scenario shaping --cols 200 --rows 60 --frames 600`, no server, synthetic Replica. Three variants, same random content generator (seed 42), each frame changes:

- **A. worst-case churn** — every cell changes every frame (random printable ASCII + 8 styles). Cache is useless. Measures raw shaping throughput.
- **B. htop-like** — 10 % of rows change per frame, rest stable. Measures cache effectiveness.
- **C. scroll** — content shifts up one row per frame. Measures whether the cache keys survive a scroll (they should, since runs are keyed by text, not position).

Record `frame_ms` p50/p99 and `shape_cache_hit_rate`.

Pass: **B and C p99 ≤ 8 ms** (half a frame, leaving room for the rest of GPUI) **and A p99 ≤ 33 ms** (a fully random 12 k-cell screen at 30 fps is acceptable since no real program does that). Fail on B/C → try per-glyph cache (Zed's approach: shape once per unique `(char, style)`, position manually) before any other M2 work; fail on A only → note it, proceed. The result and numbers go in ADR-0008.

---

## 4. CI (GitHub Actions)

### 4.1 Matrix

| Runner | Role | Must pass to close a milestone |
|---|---|---|
| `ubuntu-22.04` x64 | fmt, clippy, deny, `cargo test`, `bun test` (non-renderer), native build, nightly perf (server-only scenarios), Linux e2e experiment | yes (Q3) |
| `macos-14` arm64 | `cargo test`, `bun test` incl. `TestGpuixRenderer`, native build, e2e smoke, nightly perf (full scenarios) | yes (Q3) |
| `windows-2022` x64 | `cargo build` + native build only, `continue-on-error: true` | no (Q3, deferred) |

### 4.2 Caching

- `Swatinem/rust-cache@v2` keyed on `Cargo.lock` + `rust-toolchain.toml` + the gpuix submodule SHA, `workspaces: ". -> target"`.
- `sccache` (`mozilla-actions/sccache-action`) with GitHub Actions cache backend, `RUSTC_WRAPPER=sccache`, for the vendored Zed/GPUI object files. This is what makes the cold build survivable: the GPUI dependency graph does not change between PRs, so its objects are almost always hits.
- `oven-sh/setup-bun@v2` with `bun-version: 1.4.0`; `bun install --frozen-lockfile`; bun cache dir cached on `bun.lock`.
- A weekly **warm-cache job** on `main` (cron Sunday) that builds GPUI with sccache for all three OSes so Monday's PRs hit the cache even after GitHub's 7-day cache eviction.

### 4.3 Jobs

```yaml
jobs:
  lint:      # ubuntu, ~3 min: cargo fmt --check, clippy -D warnings (workspace, no gpui feature), cargo deny check, bun tsc, biome
  test-rust: # matrix ubuntu+macos, ~8 min warm: cargo nextest run --workspace --exclude st-native
  test-bun:  # matrix ubuntu+macos: bun test (macos also runs *.render.test.tsx)
  native:    # matrix ubuntu+macos+windows(coe): cargo build -p st-native --release, upload .node artifact
  e2e:       # macos, needs native: bun test e2e/, uploads screenshots on failure
  e2e-linux: # ubuntu, needs native, continue-on-error, Xvfb + lavapipe
  perf:      # nightly cron, macos+ubuntu, needs native: st-bench all scenarios, posts JSON to gh-pages branch + summary comment on a pinned issue
```

`test-rust` excludes `st-native` so it does not pull the GPUI build; `native` builds it once with sccache and the `gpui-test` feature tests run there.

### 4.4 Time budget and the GPUI cold build

Target: PR feedback in ≤ 12 min warm. Cold GPUI build is 15–25 min per OS (Q12 estimate 10–20, we budget the upper end). Mitigations, in order: sccache (expected cold → 4–6 min since most of the graph is cache hits); the weekly warm job; `native` and `e2e` are `needs: lint` only, so they run in parallel with tests; `lint` and `test-rust` never touch GPUI. If a PR changes `vendor/gpuix` or the Zed pin, we accept the cold build and label the PR `build:cold`. Job timeout is 60 min; a `native` job over 30 min warm is itself treated as a CI regression.

---

## 5. Local dev quality gates

- `just check` = `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, `bun run tsc --noEmit` (all workspaces), `bun test`, `cargo nextest run --workspace --exclude st-native`. Same commands as CI `lint` + `test-*`, so green locally means green in CI minus the GPU tests. Target < 3 min warm.
- `just check-native` adds `cargo test -p st-native --features gpui-test` and the macOS render tests; run before touching `st-native` or `packages/app` chrome.
- Pre-commit via `lefthook` (single binary, no Python): `cargo fmt`, `biome check --write` on staged files, and `cargo deny check licenses` only when `Cargo.lock` changed. No tests in the hook; those are `just check`.
- `cargo deny`: allowlist `Apache-2.0`, `MIT`, `BSD-2/3-Clause`, `ISC`, `Zlib`, `Unicode-3.0`, `MPL-2.0` (GPUI deps pull MPL for some font crates — confirm once the lock exists); deny `GPL-*`, `AGPL-*`, `SSPL`. Verified 2026-08-31: **gpuix is Apache-2.0** (GitHub license file); **alacritty_terminal 0.26.0 is Apache-2.0** with `rust-version = 1.85.0` (crates.io). GPUI itself is Apache-2.0 in Zed's repo but the Zed *workspace* contains AGPL/GPL crates — `cargo deny` must be scoped to what we actually link (only `gpui` and its deps), and the `deny.toml` `exceptions` section names them explicitly.
- MSRV: `rust-version = "1.85"` in the workspace `Cargo.toml` (matches alacritty_terminal; gpuix/Zed may force higher — check in M0 and bump; CI `lint` runs clippy on stable, plus a `cargo check` on the MSRV toolchain for `st-proto`, `st-core`, `st-client-core`, `st-server` only).

---

## 6. Debugging aids

- `superterminald --foreground -v[v]`: no daemonize, `tracing-subscriber` to stderr, `-vv` adds per-Delta events; `--trace-chrome out.json` enables `tracing-chrome`.
- `st dump-data <path|->`: decodes a captured data-plane stream (from `socat -u UNIX-CONNECT:$XDG_RUNTIME_DIR/superterminal/data.sock - > cap.bin`, or from a `.rec`) and prints one line per frame (`seq`, type, dirty rows, cursor); `--grid` renders the Replica after each Delta as text.
- Control plane by hand: `socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/superterminal/control.sock` then type `{"type":"ListSessions"}`; because it is newline-delimited JSON (Q14) this works without tooling. `st ctl <json>` wraps it.
- `SUPERTERMINAL_RECORD=1` (server env) writes every Surface's raw PTY output to `$XDG_STATE_HOME/superterminal/recordings/<surface-id>.rec` with a small header (`cols, rows, start time`) and resize markers; these are the `st-bench` inputs and the attachment we ask for in bug reports. `st replay <file>` plays one back into a fresh Surface in a new tab.
- `contentLines`, `styleTableSize`, `replicaSeq` debug custom props on `<terminal-grid>` (read via `getCustomProp`), enabled by `SUPERTERMINAL_DEBUG_PROPS=1` so the property snapshot cost is not paid in normal runs.
- `debugFrameOverlay: 'minimal'` toggled by a palette command *Toggle Frame Overlay* (dev builds only).

---

## 7. Definition of done per milestone

Milestone names refer to 07-milestones.md (see open question 1 if names there differ).

| Milestone | Suites that must be green |
|---|---|
| **M0 — De-risk spike** | `native` job builds on ubuntu + macos with sccache and posts the cold/warm build times; gpuix counter example runs under Bun 1.4.0 (Q36d) as a manual checklist item; WSLg Vulkan smoke noted. No test suites yet. |
| **M1 — Server + protocol** | `st-proto` all; `st-core` 1.2 all except `wide_and_zero_width_cells`; `st-server` actor/persistence/lock/idle tests; alacritty ref *engine* conformance; `lint`. |
| **M2 — Native grid renders** | M1 + `st-client-core` Replica/history tests + `prop_replica_equals_server_grid` (1 000 cases); `st-native` GPU-less tests; **shaping-cache gate passed and recorded in ADR-0008**; `cat-100mb` and `yes-10m` at 60 fps p99 in `st-bench` full mode on the dev machine. |
| **M3 — Full terminal** | M2 + key/mouse/selection encoder tests; alacritty ref *Replica* conformance; vttest checklist filed; e2e smoke on macOS; `echo-probe` < 1 frame; `st-server` backpressure tests. |
| **M4 — Sessions, tabs, chrome** | M3 + all `packages/app` tests (renderer tests on macOS); e2e detach/reattach + auto-spawn; `find-10tabs` thresholds. |
| **M5 — Reconnect polish + perf** | M4 + nightly perf green 7 consecutive nights on both OSes; RSS budgets; esctest report filed or dropped; Linux e2e promoted from `continue-on-error` if 20 nightly passes. |
| **M6 — Packaging** | M5 + e2e smoke runs against the `bun build --compile` artifact rather than `bun src/main.tsx`; `cargo deny` clean on the release lock. |

---

## Open questions

1. **Milestone names.** `07-milestones.md` does not exist yet; the M0–M6 names above are proposed from the grilling doc's references (M0 de-risk, M2 shaping gate, M3 vttest, M6 packaging). 07 should adopt or rename them and this table follows.
2. **gpuix test renderer API names.** The brief names `TestGpuixRenderer.applyBatch/flush/drainEvents/simulateKeystrokes/simulateClick/getCustomProp`; the current README documents `createTestRoot()`, `renderer.flush()`, `drainNativeEvents()`, `nativeSimulateClick`, `nativeSimulateKeystrokes`, `getPaintedText()`, and does **not** document `getCustomProp`. Confirm against the pinned gpuix commit in M0; `contentLines` may need to be exposed as painted text or a `<text testId="readout">` instead.
3. **Linux headless rendering.** gpuix says the test renderer is "not yet" on Linux, and GPUI needs Vulkan. Whether Xvfb + lavapipe works is unknown; this plan puts e2e on macOS and treats Linux e2e as an experiment. That mildly conflicts with Q3 ("Linux primary for development") — a Linux dev cannot run render tests locally without a GPU. Accept, or invest M3 time in the lavapipe path?
4. **`gpui::TestAppContext` outside Zed's workspace.** Assumed usable through the vendored path dependency with the `test-support` feature; unverified.
5. **Frame-size cap.** `MAX_FRAME = 16 MiB` is proposed here; 02-protocol.md owns the number. A 200×100 Snapshot with 10 k history ids is well under 1 MiB, so the cap is generous.
6. **Perf regression policy.** "> 15 % worse than 7-day median" vs. absolute Q27 numbers on CI runners: the plan uses relative on CI, absolute on dev machines. Confirm this is acceptable as the M5 gate definition.
7. **Vendoring alacritty ref fixtures vs. fetching at test time.** Vendoring (~ a few MB) keeps tests offline and pinned; fetching keeps the repo small. Plan says vendor.
8. **MPL-2.0 in the allowlist** depends on what GPUI actually links; decide after the first `cargo deny` run in M0.
