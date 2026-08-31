# 03 — Server (`superterminald`: crates/st-server + crates/st-core)

> **Addendum (00-grilling §F):** Q37 the DATA plane opens with a 4-byte magic (sniffing ambiguity resolved); Q42 *pristine* Surfaces count as zero for idle exit; Q40 last-resize-wins, reflow off, selection cleared on resize; Q39 Deltas carry `history_len`; Q45 style table cap 4 096; Q46 config parsing lives in `st-config`, the CLI in `st-cli`; Q48 OSC 10/11 answered from `[theme]`, OSC 52 off, `alacritty_terminal` from crates.io 0.26.x, default Session `Default`, env allow-list on `CreateSurface`.

Plan only. Relies on the frozen decisions in `00-grilling.md` (Q6–Q9, Q16–Q18, Q21–Q22, Q27, Q30–Q31, Q34). Wire formats live in `02-protocol.md`; this document uses its message names (`Hello`, `Attach`, `Snapshot`, `Delta`, `Ack`, `Input`, `FetchHistory`, `SetSelection`) without redefining them. API facts below were checked against `alacritty_terminal` 0.26.0 (re-exporting `vte` 0.15 with the `ansi` feature) and `portable-pty` 0.9.0.

## 1. Responsibilities and non-responsibilities

The daemon **owns**: every PTY and child process (Q9); one authoritative VT state machine per Surface (Q7); the Workspace document — Sessions → Tabs → Surfaces plus per-Surface `ViewState` (Q17); production of `Snapshot`/`Delta` streams per attached client (Q16, Q27); persistence of the Workspace *shape* to `workspace.json` (Q18); its own lifecycle (Q30).

The daemon **does not**: render anything, know about fonts, cell pixel sizes or themes beyond answering OSC colour queries; encode keystrokes (clients send already-encoded `Input` bytes, Q23); compute selections (clients compute, server stores, Q24); speak TCP; run any JavaScript.

Consequence: `st-core` has zero dependencies on tokio, gpuix or Bun and is fully unit-testable; `st-server` is the thin async shell around it.

## 2. Process lifecycle

**Paths.** Runtime dir `$XDG_RUNTIME_DIR/superterminal/` (fallback `$TMPDIR/superterminal-<uid>/` on macOS), created `0700`, containing `sock` and `lock`. State dir `$XDG_STATE_HOME/superterminal/` (fallback `~/.local/state/superterminal/`) holds `workspace.json` and `logs/`. Config from `~/.config/superterminal/config.toml` (Q34); the server reads only `[server]` and `[shell]` tables plus `[theme]` for OSC colour replies.

**Startup sequence.**
1. Parse CLI (`run` default, `status`, `stop`; flags `--foreground`, `--socket <path>`, `--state-dir <path>`, `--no-idle-exit`, `--log-level`).
2. Init logging (§2 Logging).
3. Open `lock`, `flock(LOCK_EX | LOCK_NB)`. Failure ⇒ another instance is alive ⇒ log at `info`, exit 0 (this is the normal outcome of the client's spawn race, Q30). The fd is held for the life of the process; the pid is written into it for `status`.
4. Unlink a stale `sock` if present (we hold the lock, so nobody else can be serving it), `tokio::net::UnixListener::bind`, `chmod 0600`.
5. Load `workspace.json` (§8). Corrupt ⇒ rename to `workspace.json.corrupt-<ts>`, start with one Session containing one Tab.
6. Re-seed: spawn a Surface for every saved Tab in its saved cwd (falling back to `$HOME` if the directory is gone). Eager, not lazy: Q18 says the fresh server recreates the shape.
7. Enter accept loop. Readiness is implicit — the client retries connect for 3 s (Q30).

**Single instance.** The `flock` is the guarantee; the socket path is derived, never configurable except via `--socket` for tests (which also switches the lock path).

**Idle shutdown (Q30).** A timer arms when the last connection closes; it is cancelled by any accept. When it fires after `server.idle_exit_minutes` (default 15) *and* there are zero *live* Surfaces, the server performs a graceful shutdown. Because Q21 re-seeds the last Session with a fresh Tab, "zero Surfaces" is refined to "zero non-pristine Surfaces", where *pristine* = shell spawned by re-seed, never received `Input`, child still the original shell (see Open questions). `--no-idle-exit` disables the timer.

**Signals.**
- `SIGTERM`/`SIGINT`: graceful. Stop accepting; flush `workspace.json` immediately (bypass debounce); send every connection a shutdown notice (message per `02-protocol.md`); `killpg(pgid, SIGHUP)` each Surface; wait ≤2 s; `SIGKILL` survivors; unlink `sock`; release lock; exit 0.
- `SIGHUP` to the daemon: ignored (the daemon is detached; it must survive the spawning terminal).
- `SIGPIPE`: ignored (Rust runtime default).
- `SIGCHLD`: not handled; each child has a blocking `wait()` thread (§4).

**Logging.** `tracing` + `tracing-subscriber` (`EnvFilter` from `SUPERTERMINAL_LOG` or `--log-level`, default `info`) + `tracing-appender` daily rolling file `logs/superterminald.log`, keep 7 files. Spans: `surface{id}`, `conn{id, plane}`. `--foreground` additionally writes pretty logs to stderr; it does not change daemonisation because the server never daemonises itself — the client spawns it detached (Q30). Never log PTY bytes above `trace`.

## 3. Domain model (st-core)

```rust
#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct SessionId(pub u64);
pub struct TabId(pub u64);       // same derives
pub struct SurfaceId(pub u64);
pub struct ClientId(pub u64);    // per-connection, never persisted

pub struct Workspace {
    pub next_id: u64,                      // one counter for all id kinds
    pub sessions: Vec<Session>,            // ordered
    pub active_session: SessionId,
}
pub struct Session { pub id: SessionId, pub name: String, pub tabs: Vec<Tab>, pub active_tab: TabId }
pub struct Tab     { pub id: TabId, pub surface: SurfaceId }   // exactly one Surface in v1 (Q19)
pub struct Surface {
    pub id: SurfaceId,
    pub title: String,               // from OSC 0/2, else shell name
    pub cwd: PathBuf,                // OSC 7 or probed (§9)
    pub shell: PathBuf,
    pub status: SurfaceStatus,
    pub view: ViewState,
    pub size: (u16 /*cols*/, u16 /*rows*/),
    pub pristine: bool,              // §2 idle rule
}
pub enum SurfaceStatus { Running { pid: u32 }, Exited { code: Option<i32>, signal: Option<i32> } }
pub struct ViewState { pub scroll_offset: u32, pub selection: Option<Selection> }
pub struct Selection { pub start: AbsPoint, pub end: AbsPoint, pub kind: SelectionKind /*Simple|Block|Lines*/ }
pub struct AbsPoint { pub line: AbsLine /*u64, §4 line ids*/, pub col: u16 }
```

`Workspace` exposes pure mutation methods (`create_session`, `create_tab(session, seed)`, `close_tab`, `move_tab`, `rename_session`, `set_view_state`, `set_surface_status`, …) each returning a `Vec<WorkspaceEvent>`; the Q21 rules (closing the last tab deletes the Session, the last Session re-seeds) are implemented here as pure functions and property-tested.

**Single-writer actor.** In `st-server`, exactly one task owns a `Workspace`:

```rust
pub enum WsCommand { Apply { op: WorkspaceOp, reply: oneshot::Sender<Result<Vec<WorkspaceEvent>>> },
                     Snapshot { reply: oneshot::Sender<WorkspaceView> }, /* … */ }
pub struct WorkspaceActor { ws: Workspace, surfaces: HashMap<SurfaceId, SurfaceHandle>,
                            cmds: mpsc::Receiver<WsCommand>, events: broadcast::Sender<(u64 /*ws_seq*/, WorkspaceEvent)>,
                            persist: PersistDebouncer }
```

Control connections send `WsCommand`s over a bounded `mpsc` and subscribe to the `broadcast` to echo changes to every client (Q17). Surface actors report `TitleChanged`, `CwdChanged`, `Exited`, `Bell` upward through the same command channel.

Why not `Arc<Mutex<Workspace>>`: (a) every mutation must be *ordered* and stamped with `ws_seq` so client projections converge — a channel gives a total order for free; a mutex does not order the broadcast that follows the unlock. (b) Mutations spawn PTYs and send to Surface actors — holding a lock across `await`s or nesting it with per-Surface locks is a deadlock farm. (c) The debounced persister and the idle timer need "the state as of one instant"; the actor hands out immutable `WorkspaceView` clones. (d) Deterministic tests: feed commands, assert events.

## 4. Surface engine (alacritty-backed)

Each Surface is its own tokio task owning:

```rust
struct SurfaceActor {
    engine: Box<dyn VtEngine>,           // §5; AlacrittyEngine in v1
    pty: PtyHandle,                      // master + writer channel
    publisher: Publisher,                // §6
    cmds: mpsc::Receiver<SurfaceCmd>,    // Input, Resize, Attach, Detach, Ack, FetchHistory, Kill
    pty_rx: mpsc::Receiver<Bytes>,       // from the blocking reader thread
    exit_rx: oneshot::Receiver<ExitStatus>,
}
```

**PTY I/O.** `native_pty_system().openpty(PtySize{rows, cols, pixel_width: 0, pixel_height: 0})` → `PtyPair{master, slave}`. `master.try_clone_reader()` feeds a dedicated `std::thread` reading 64 KiB chunks into a *bounded* (`cap 16`) `mpsc<Bytes>` — when the actor falls behind, the reader blocks, the kernel PTY buffer fills, and the shell blocks on `write(2)`: correct end-to-end backpressure, same shape as alacritty's own `event_loop`. `master.take_writer()` goes to a second thread fed by an `mpsc<Bytes>` for `Input` and for VT replies. A third thread blocks in `child.wait()` and reports `ExitStatus` (portable-pty `Child::wait(&mut self) -> IoResult<ExitStatus>`). Reader EOF/`EIO` after exit ⇒ drain, then status `Exited` (Q22 — grid stays, nothing auto-closes).

**Parsing.** `Term<Listener>` implements `vte::ansi::Handler`, so the actor does literally `self.processor.advance(&mut self.term, &chunk)` with `processor: vte::ansi::Processor` (default `StdSyncHandler` timeout for synchronized updates). `Listener` implements `EventListener::send_event(&self, Event)` by pushing into a `std::sync::mpsc::Sender<Event>` drained after each `advance`:
- `Title(String)`/`ResetTitle` → title in the next Delta + `TitleChanged` to the Workspace actor.
- `Bell` → one-shot `bell` flag in the next Delta (coalesced by OR).
- `PtyWrite(String)`, `ColorRequest(idx, f)`, `TextAreaSizeRequest(f)` → the reply string goes to the PTY writer; colours come from `[theme]` in config or the xterm defaults (Open questions).
- `ClipboardStore/Load` → forwarded to attached clients only if `osc52` is enabled in config; default off in v1.
- `Wakeup`, `MouseCursorDirty`, `CursorBlinkingChange`, `Exit`, `ChildExit` → ignored (we own the process, not alacritty's `event_loop`).

**Damage → dirty rows.** After each `advance` (or batch of chunks in one tick): `match term.damage() { TermDamage::Full => dirty.set_all(), TermDamage::Partial(it) => for LineDamageBounds{line, ..} in it { dirty.set(line) } }` then `term.reset_damage()`. We deliberately drop `left/right` — Q16 is row-granular. `dirty` is a `DirtySet` bitset of `screen_lines` bits.

**Cells → packed cells.** A row is materialised at *send* time (§6) from `term.grid()[Line(i)]`: for each `Cell{c, fg, bg, flags, extra}` produce `PackedCell{ codepoint: u32, style: u16, flags: u8 }` (Q16). Cell flags kept in the `u8`: `WIDE_CHAR`, `WIDE_CHAR_SPACER`, `LEADING_WIDE_CHAR_SPACER`, `HIDDEN`; zero-width combining chars (`extra.zerowidth()`) are appended as a side list per row so the packed cell stays 7 bytes. Everything visual goes through the style table.

**StyleTable interning.** Key:

```rust
struct StyleKey { fg: Col, bg: Col, ul: Col, attrs: u16 }   // attrs: BOLD DIM ITALIC INVERSE STRIKEOUT + underline kind (none/single/double/curl/dotted/dashed)
enum Col { DefaultFg, DefaultBg, Indexed(u8), Rgb(u8,u8,u8) }   // symbolic, so the client theme applies (Q26/Q34)
struct StyleTable { by_key: HashMap<StyleKey, u16>, entries: Vec<StyleKey>, new_since_flush: u16, generation: u32 }
```

`intern(key) -> u16`: hash lookup; miss ⇒ push, return index, and record it in `new_since_flush`. Every Delta/Snapshot/FetchHistory response carries the entries added since the previous message on that subscription *before* any row that references them, so a client can apply in one pass. Alacritty `Color::Named` maps to `DefaultFg/DefaultBg` or `Indexed(0..16)`; `Color::Indexed(n)` → `Indexed(n)`; `Color::Spec(rgb)` → `Rgb`. Index 0 is always the default style.

*Eviction/reset policy.* The table is append-only until `entries.len() >= 60_000` (headroom under the `u16` ceiling), or until the Surface is reset (`RIS`, "Clear Scrollback" command). Then: `generation += 1`, clear both maps, re-intern index 0, and force the next message on every subscription to be a full `Snapshot` (which carries the whole table). Ordering by `seq` makes this safe: nothing after the reset references old indices, and history rows are never stored with indices — `FetchHistory` re-encodes from alacritty cells at request time, so evicted indices cannot leak.

**Cursor & modes.** From `term.renderable_content()`: `cursor: RenderableCursor{shape, point}` and `mode: TermMode`. We export `SHOW_CURSOR`, `ALT_SCREEN`, `BRACKETED_PASTE`, `MOUSE_REPORT_CLICK | MOUSE_DRAG | MOUSE_MOTION` (collapsed to a 2-bit mouse mode), `SGR_MOUSE`, `UTF8_MOUSE`, `APP_CURSOR`, `APP_KEYPAD`, `FOCUS_IN_OUT`, `ALTERNATE_SCROLL`, `KITTY_KEYBOARD_PROTOCOL` bits, plus `term.cursor_style()` for blink.

**Scrollback.** `term::Config{ scrolling_history: server.scrollback_lines (default 10_000, max 100_000), kitty_keyboard: true, osc52: Osc52::Disabled, .. }`. The alt screen has no history in alacritty; while `ALT_SCREEN` is set, `Snapshot.history_len = 0` and `scrollback_appended = 0`; entering/leaving the alt screen yields `TermDamage::Full`, so the transition is always a full-screen Delta with the flipped mode bit. Snapshots always serialise the *active* grid's visible rows.

**Resize.** `SurfaceCmd::Resize{cols, rows}` → `master.resize(PtySize{..})` (the kernel raises `SIGWINCH` on `TIOCSWINSZ`) then `term.resize(TermSize::new(cols, rows))`; alacritty marks full damage. v1 policy: last resize wins for all attached clients (Open questions).

**Stable absolute line ids (Q16/Q18 hooks).** Alacritty addresses lines relative to the viewport (`Line(0)` = top of screen, negative = history) and its ring evicts silently. We keep `AbsLine(u64)`: `first_history_abs` = number of lines ever evicted from the ring; the absolute id of grid `Line(l)` is `first_history_abs + (l + history_size) as u64`. Growth while under the cap is measured as the `history_size()` delta around `advance`; at the cap, eviction is counted by a `Handler` shim (`struct Counting<'a>(&'a mut Term<L>)`) that intercepts `linefeed`/`newline`/`scroll_up` and increments when the cursor sits on the last line of the (whole-screen) scroll region, delegating everything else. A proptest checks shim counts against `total_lines()` below the cap. `AbsLine` is what `Selection`, `FetchHistory{from,count}` and `Snapshot.history_len` speak; it survives client reconnects (Q17) but not server restarts (Q18 — history is not persisted).

## 5. VtEngine trait boundary (Q8)

```rust
pub trait VtEngine: Send {
    fn advance(&mut self, bytes: &[u8]);
    fn drain_events(&mut self) -> Vec<VtEvent>;          // Title, Bell, PtyReply(Vec<u8>), Clipboard…
    fn take_damage(&mut self) -> Damage;                  // Damage::Full | Damage::Rows(DirtySet); resets internal damage
    fn snapshot(&self, styles: &mut StyleTable) -> Snapshot;
    fn row(&self, line: u16, styles: &mut StyleTable) -> PackedRow;
    fn cursor_and_modes(&self) -> (CursorState, Modes);
    fn resize(&mut self, cols: u16, rows: u16);
    fn history_len(&self) -> u64;                          // in AbsLine units
    fn history_lines(&self, from: AbsLine, count: u32, styles: &mut StyleTable) -> Vec<PackedRow>;
    fn reset(&mut self);                                   // RIS / clear scrollback
}
```

`AlacrittyEngine` (st-core `vt/alacritty.rs`) is the only implementation in v1; a `GhosttyEngine` would live behind a cargo feature and be selected by config. Everything above the trait (packing, interning, publishing) must not import `alacritty_terminal`. Note `Damage::Full` ⇒ `DirtySet::all()` — the trait never exposes column bounds.

## 6. Delta production and fan-out

```rust
pub struct Publisher { seq: u64, subs: HashMap<ClientId, Subscription>, tick: Option<Sleep>, last_flush: Instant }
pub struct Subscription {
    tx: mpsc::Sender<DataFrame>,            // bounded, per connection
    last_sent_seq: u64, last_acked_seq: u64,
    pending: Coalesced,                     // dirty: DirtySet, cursor_dirty, modes_dirty, title_dirty, bell: bool, scrollback_appended: u32
    needs_snapshot: bool, stalled_since: Option<Instant>,
}
```

**Coalescing.** On every `take_damage`, the actor ORs the dirty set into *each* subscription's `pending` and adds `scrollback_appended`. Row *content* is not captured at damage time — it is read from the engine when the frame is built, so a Delta always carries the latest state ("coalesced final state", Q27). Cursor, modes and title are likewise read at build time. `bell` is OR. Memory per subscription is therefore bounded by one bitset plus a few scalars regardless of how far behind the client is.

**Timer.** 120 Hz means a minimum inter-flush gap of 8.33 ms per Surface. Leading-edge: if `now - last_flush >= gap`, flush synchronously right after `advance` (keeps input-to-glyph latency at one hop). Otherwise arm a single `tokio::time::Sleep` for `last_flush + gap`; the tick flushes and disarms. No idle ticks.

**Flush.** For each subscription with a non-empty `pending`: if `last_sent_seq - last_acked_seq >= ack_window` (default 4) skip — pending keeps accumulating. If `needs_snapshot`, build `Snapshot` (all rows, full style table, history_len, cursor, modes, title, view state), else build `Delta{seq, rows, new_styles, cursor, modes, title, scrollback_appended, bell}`. `try_send` into the connection's bounded channel; a full channel counts as "not sent" (no `await` on a slow socket inside the Surface actor).

**Backpressure / slow-client policy.** The client sends `Ack{seq}` for the latest Delta applied. Stalled subscriptions (window full) never buffer frames; they coalesce forever. If a subscription has been window-blocked for `slow_client_snapshot_secs` (default 3), set `needs_snapshot = true` — cheaper than replaying and guarantees convergence. If no `Ack` arrives for 30 s the connection is closed (its Surfaces stay attached-free, nothing else happens). `FetchHistory` responses bypass the window (they are request/response) but go through the same bounded channel.

**Attach.** `Attach{surface}` creates the Subscription with `needs_snapshot = true`; the first flush is immediate. `Detach` or connection close removes it. Multiple attaches from one client to the same Surface are rejected.

## 7. Connection handling

The accept loop (`UnixListener::accept()` → `(UnixStream, _)`) spawns one task per connection with a 5 s handshake timeout. First it checks `stream.peer_cred()?.uid() == getuid()` (defence in depth over the `0600` socket). Then it peeks the first 4 bytes: `b'{'` ⇒ CONTROL (newline-delimited JSON, Q14); anything else ⇒ DATA (`u32 len | u16 type | payload`, Q15). The bytes are pushed back into the framed reader. Both planes must open with `Hello{proto_version, build_id}` (Q31); a lower major version gets a readable refusal and a close. Connections beyond `server.max_connections` (default 64) receive a refusal and are closed.

CONTROL task: parse a line → `WsCommand` → reply line; concurrently forward `broadcast` Workspace events as lines (`recv` lag ⇒ resend a full `WorkspaceView`). DATA task: demux frames by `SurfaceId` to Surface actors (`Attach/Detach/Input/Ack/FetchHistory/Resize/SetSelection`), and drain its outbound `mpsc<DataFrame>` to the socket. On close, every Surface the connection had attached receives `Detach`. Auth is filesystem permissions: runtime dir `0700`, socket `0600`, plus the uid check.

## 8. Persistence

`$XDG_STATE_HOME/superterminal/workspace.json`:

```json
{ "version": 1, "saved_at": "2026-08-31T12:00:00Z", "next_id": 42, "active_session": 1,
  "sessions": [ { "id": 1, "name": "Work", "active_tab": 7,
      "tabs": [ { "id": 7, "surface": { "id": 8, "cwd": "/home/sonny/projects/x", "shell": "/bin/zsh", "title": "zsh" } } ] } ] }
```

Written by the Workspace actor through `PersistDebouncer`: 500 ms trailing debounce after any structural or cwd/title event, immediate on `SIGTERM`. Write is `workspace.json.tmp` → `fsync` → `rename` (atomic). Unknown `version` ⇒ treat as corrupt (§2). **Not persisted:** grid contents and scrollback, `ViewState` (meaningless for a fresh shell), pids, `SurfaceStatus`, seq counters, style tables, connection state, terminal size.

## 9. Spawning the shell

Resolution: `[shell].program` (+ `args`) from config → `$SHELL` → passwd entry (`CommandBuilder::get_shell()` does the last two) → `/bin/sh`. `[shell].login` defaults to `true` on macOS, `false` on Linux; when true, `-l` is appended for `bash`/`zsh`/`fish` (not for `sh`/`dash`).

Environment: inherit the daemon's env (captured when the client spawned it — Open questions), then set `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=superterminal`, `TERM_PROGRAM_VERSION=<build_id>`, `SUPERTERMINAL_SURFACE_ID=<u64>`, `SUPERTERMINAL_SOCKET=<path>`; remove `TMUX`, `STY`, `TERM_SESSION_ID`. Rejected requests to override `TERM` are logged.

Cwd for a new Tab is the *current* cwd of the Session's active Surface: (1) OSC 7 `file://host/path` tracked by `Osc7Sniffer`, a second `vte::Parser` + `Perform` impl that only implements `osc_dispatch` (alacritty's `Handler` never sees OSC 7, so it cannot be intercepted through `Term`); (2) fallback probe of the foreground process: `master.process_group_leader()` (`tcgetpgrp`) → Linux `readlink /proc/<pgid>/cwd`, macOS `proc_pidinfo(pgid, PROC_PIDVNODEPATHINFO)`; (3) the Surface's spawn cwd. The probe also runs every 2 s while at least one client is attached so `workspace.json` stays fresh.

`slave.spawn_command(cmd)` with `set_controlling_tty(true)` (default) gives the child its own session, process group and controlling tty. Kill (Q21): `killpg(pid, SIGHUP)`, 2 s grace, `SIGKILL`, then drop the master.

## 10. Security

Unix socket only, `0600` inside a `0700` directory, `SO_PEERCRED` uid check; no TCP, no environment variable can turn it on in v1. Control messages carry ids, never paths, except `cwd` for new Surfaces — validated to be an absolute existing directory; no traversal surface exists because nothing is served from a path. Untrusted bytes (PTY output) only ever reach `vte`, which is fuzzed upstream. Future remote mode (Q5 hook): the same two planes over an SSH-forwarded socket or a mux binary; nothing here assumes locality except the cwd probe and `peer_cred`.

## 11. Observability

`superterminald status` connects on CONTROL, sends the status request (name per `02-protocol.md`) and prints JSON: pid, uptime, socket path, connection counts per plane, Surfaces (id, pid, status, size, subscribers), and metrics. `superterminald stop` sends the shutdown request (same effect as `SIGTERM`).

`Metrics` is a struct of `AtomicU64`s in `st-server::metrics`: `pty_bytes_in`, `pty_bytes_out`, `frames_out`, `deltas_sent`, `snapshots_sent`, `damage_events`, `flushes` (coalesce ratio = `damage_events / deltas_sent`), `window_blocked_flushes`, `forced_snapshots`, `connections_accepted/refused`, `persist_writes`. Sampled into the log every 60 s at `debug`.

## 12. Module layout

**crates/st-core** (no tokio, no I/O except syscalls in `cwd`)
- `lib.rs` — re-exports.
- `ids.rs` — `SessionId`, `TabId`, `SurfaceId`, `ClientId`, `IdGen`.
- `workspace.rs` — `Workspace`/`Session`/`Tab`/`Surface`, `WorkspaceOp`, `WorkspaceEvent`, Q21 rules.
- `view_state.rs` — `ViewState`, `Selection`, `AbsPoint`, `AbsLine`.
- `cell.rs` — `PackedCell`, `PackedRow`, alacritty `Cell` → packed conversion.
- `style.rs` — `StyleKey`, `Col`, `StyleTable` interning and reset policy.
- `delta.rs` — `DirtySet`, `Coalesced`, builders for `Snapshot`/`Delta` payloads (using `st-proto` types).
- `publisher.rs` — `Publisher`/`Subscription` state machine as pure functions over an injected clock.
- `vt/mod.rs` — `VtEngine`, `Damage`, `VtEvent`, `CursorState`, `Modes`.
- `vt/alacritty.rs` — `AlacrittyEngine`: `Term<Listener>` + `Processor`, event drain, damage, resize.
- `vt/line_ids.rs` — absolute line accounting and the counting `Handler` shim.
- `vt/osc7.rs` — `Osc7Sniffer` (`vte::Perform`).
- `shell.rs` — shell resolution, login rule, environment construction.
- `persist.rs` — `WorkspaceFile` v1 schema, load/validate/serialise, `Workspace` ⇄ file mapping.
- `config.rs` — `[server]`/`[shell]`/`[theme]` subsets of `config.toml`.

**crates/st-server** (binary `superterminald`)
- `main.rs` — clap CLI: `run`, `status`, `stop`, flags.
- `paths.rs` — XDG/runtime/state directory resolution and creation with modes.
- `lock.rs` — `flock` lockfile with pid.
- `logging.rs` — tracing setup, rolling appender, `--foreground` stderr layer.
- `signals.rs` — SIGTERM/SIGINT/SIGHUP handling.
- `daemon.rs` — startup sequence, re-seed, idle timer, graceful shutdown orchestration.
- `workspace_actor.rs` — the single-writer actor, broadcast, `PersistDebouncer`, `SurfaceHandle` registry.
- `surface/mod.rs` — `SurfaceActor` task loop and `SurfaceCmd`.
- `surface/pty.rs` — portable-pty open/spawn/resize/kill, reader and writer threads.
- `surface/exit.rs` — child waiter thread → `ExitStatus`.
- `surface/cwd.rs` — Linux `/proc` and macOS `proc_pidinfo` probes.
- `net/listener.rs` — accept loop, `peer_cred`, plane sniffing, connection limits.
- `net/control.rs` — NDJSON control connection task.
- `net/data.rs` — framed data connection task, per-Surface demux, outbound drain.
- `metrics.rs` — atomic counters and periodic dump.
- `status.rs` — client side of `status`/`stop`.
- `tests/pty_echo.rs`, `tests/persist_roundtrip.rs`, `tests/reconnect_snapshot.rs`, `tests/slow_client.rs` — integration tests over `--socket` in a temp dir.

## 13. Open questions

1. **Idle exit vs. re-seed (Q30 × Q21).** Because the last Session always re-seeds a Tab, "zero Surfaces" is never literally true. Proposed: count *pristine* Surfaces as zero (§2). Needs sign-off.
2. **Plane sniffing ambiguity.** A DATA frame whose `u32 len` low byte is `0x7B` (`{`) would be mis-sniffed as CONTROL. Request to `02-protocol.md`: a 4-byte magic (e.g. `STDP`) as the first bytes of the data plane, or CONTROL also opening with a magic line.
3. **Multiple clients, different sizes.** v1 = last resize wins. Alternatives (min of all, per-client letterboxing) deferred; the client doc should decide how a non-matching replica is drawn.
4. **History resync signal.** `scrollback_appended: u32` (Q16) cannot express alt-screen transitions or ring eviction to a client whose cached history has gaps. Proposal for `02-protocol.md`: Deltas also carry absolute `history_len: u64` (`AbsLine`).
5. **Eviction counting at the cap** relies on a `Handler` shim heuristic (§4). If the proptest shows drift, fallback is a ~20-line patch to `alacritty_terminal` exposing an evicted-lines counter (vendored like gpuix), or exposing it only through `VtEngine`.
6. **OSC colour queries** (`ColorRequest`, OSC 10/11) — the server has no theme knowledge by design, yet programs ask. Proposed: server reads `[theme]` from `config.toml` purely to answer queries; the client remains the renderer.
7. **Stale daemon environment.** The daemon's env is frozen at first spawn; new shells inherit it. Option: `CreateSurface` on CONTROL may carry env overrides from the client. Protocol decision.
8. **Default Session name** when starting fresh (`"Default"`?), and whether `SurfaceStatus::Exited` should be persisted as "do not re-seed this tab".
9. **`osc52` clipboard** — off in v1 (§4); on-with-confirmation later? UX call for `05-client-app.md`.
10. **Ack window / thresholds** (`ack_window = 4`, `slow_client_snapshot_secs = 3`, 30 s disconnect) are proposals to be tuned by the perf harness in `06-testing-perf.md`.
