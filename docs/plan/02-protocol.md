# 02 — Wire Protocol (v1.0)

> **Addendum (00-grilling §F):** Q37 DATA magic `0xFF"STD"` confirmed; Q38 adds `since_seq` to `Delta` and removes standalone `ModeChanged`/`TitleChanged`; Q39 replaces `scrollback_appended` with absolute `history_len: u64`; Q40 disables reflow and clears selection on resize; Q41 adopts trailing-blank trimming and the per-row `wrapped` flag; Q44 adds `Attach.mode: Active | Passive`; Q45 caps the style table at 4 096 with reset→Snapshot; Q48 adds `tab.set_active`, `cwd`/`has_foreground_child` in `SurfaceStatus`, env allow-list on `CreateSurface`, per-message `DataError`. Apply these over the text below.

Status: planning spec. Implements the frozen decisions of `00-grilling.md` (Q7, Q13–Q18, Q21–Q25, Q27, Q30–Q31). Nothing here re-decides; conflicts and gaps are collected in the final section. Rust types live in crate `st-proto`; TS types live in `packages/protocol-ts` and are generated from the same source of truth (see §10).

Conventions: all integers little-endian on the wire; `Vec<T>` is postcard's varint-length-prefixed sequence; ids are `u32`, allocated by the server, never reused during a server lifetime (they fit JS `number` exactly).

---

## 1. Transport

### 1.1 Socket

One Unix domain socket per user per server (Q30):

| Platform | Path |
|---|---|
| Linux | `$XDG_RUNTIME_DIR/superterminal/server.sock` (fallback if unset: `/tmp/superterminal-$UID/server.sock`, dir mode `0700`) |
| macOS | `~/Library/Application Support/superterminal/server.sock` |

A lockfile `server.lock` sits beside the socket (flock'd by the daemon). The socket directory is created `0700`; the server refuses to start if it is group/world accessible. No TCP listener in v1; the framing is transport-agnostic so a future SSH tunnel (Q5 hook) just forwards the socket.

### 1.2 Two connection kinds

Every connection is either CONTROL or DATA for its whole lifetime (Q14). The server classifies by the **first byte**:

- `0x7B` (`{`) → **CONTROL**. Newline-delimited JSON, UTF-8, exactly one message per `\n`-terminated line (no embedded raw newlines; `\r` is not allowed). **Max line size: 4 MiB**; a longer line closes the connection with `Reject{reason:"line_too_long"}` if possible. Debuggable with `socat - UNIX-CONNECT:…`.
- `0xFF` → **DATA**. The client sends the 4-byte magic `FF 53 54 44` (`0xFF "STD"`), then binary frames. `0xFF` never occurs in valid UTF-8, so the two kinds can never be confused.

Anything else → the server closes the connection.

### 1.3 DATA framing

```
+----------------+----------------+------------------------+
| u32 len  (LE)  | u16 msg_type   | payload (postcard)     |
+----------------+----------------+------------------------+
  len = 2 + payload.len()   (header's own 4 bytes excluded)
```

- **Max frame size: 8 MiB** (`len ≤ 8·2^20`). Larger → connection closed. A 500×200 Snapshot of wide, styled glyphs is well under 1 MiB (§11), so this is a sanity bound, not a design limit.
- `msg_type` is a `u16` from the table in §4. Unknown `msg_type` **within the negotiated version** → `Reject` and close. Unknown types above the negotiated minor's range must not be sent (see §10).
- Payload is the postcard encoding of exactly one struct from §4. Postcard is not self-describing, so struct layout is fixed per (major, minor) — the versioning rules in §10 exist for this reason.
- Frames are never interleaved; one frame completes before the next begins. Both directions are independent streams.

The Bun side never opens a DATA connection (Q13); the Rust native module never opens a CONTROL connection. Ordering between the two is guaranteed by usage, not by the protocol: JS creates a Surface via CONTROL and only afterwards mounts `<terminal-grid surfaceId>`, which triggers `Attach` on DATA.

---

## 2. Handshake

Both kinds start with a Hello exchange; the shapes are the same, only the codec differs (JSON on CONTROL, postcard on DATA with `msg_type = 0x0001/0x0002/0x0003`).

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProtoVersion { pub major: u8, pub minor: u8 }   // wire: u16 = major<<8 | minor
pub const PROTO_VERSION: ProtoVersion = ProtoVersion { major: 1, minor: 0 };

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind { Control, Data, Tool }               // Tool = CLI/inspector, control-only

#[derive(Serialize, Deserialize)]
pub struct Hello {
    pub proto_version: ProtoVersion,
    pub client_kind: ClientKind,
    pub build_id: String,        // git sha + dirty flag, informational
}

#[derive(Serialize, Deserialize)]
pub struct HelloAck {
    pub proto_version: ProtoVersion,   // the version the server will speak (see §10)
    pub server_build_id: String,
    pub workspace_revision: u64,       // current Workspace document revision
    pub server_pid: u32,
}

#[derive(Serialize, Deserialize)]
pub struct Reject {
    pub reason: RejectReason,
    pub message: String,               // human-readable, shown in the client banner (Q31)
    pub server_version: ProtoVersion,
}

#[derive(Serialize, Deserialize)]
pub enum RejectReason { MajorMismatch, BadMagic, LineTooLong, FrameTooLarge, NotHello, ShuttingDown }
```

Rules (Q31):
1. Client sends `Hello` first; the server sends nothing before receiving it. A 5 s handshake timeout closes idle connections.
2. `major` must be **equal**. Otherwise the server sends `Reject{MajorMismatch}` and closes. The client shows the banner with the "Restart server (kills running processes)" action regardless of which side is newer.
3. Negotiated `minor = min(client.minor, server.minor)`, echoed in `HelloAck`. Both sides must then restrict themselves to that minor (§10).
4. `build_id` is never used for decisions; it is logged and shown in `server.status`.

On CONTROL the handshake messages are `{"t":"hello",…}` / `{"t":"hello.ack",…}` / `{"t":"reject",…}` without `id`.

---

## 3. CONTROL messages (JSON)

### 3.1 Envelope

- **Request** (client→server): `{"t": "<name>", "id": <u32>, ...fields}`. `id` is client-chosen, unique per connection while outstanding.
- **Response**: `{"t": "ok", "id": n, "result": …}` or `{"t": "err", "id": n, "error": {...}}`. Exactly one response per request, in any order.
- **Event** (server→client, unsolicited): `{"t": "ev.<name>", ...fields}`, no `id`. Events are only sent after `workspace.subscribe`.

Error envelope:

```ts
export type ErrorCode =
  | 'bad_request'      // malformed / unknown t / missing field
  | 'not_found'        // unknown session/tab/surface id
  | 'conflict'         // if_revision did not match current workspace revision
  | 'spawn_failed'     // PTY/shell could not start; message has errno text
  | 'unsupported'      // message exists but not in the negotiated minor
  | 'shutting_down'
  | 'internal';
export interface ErrorBody { code: ErrorCode; message: string; data?: unknown }
export interface ErrRes { t: 'err'; id: number; error: ErrorBody }
export interface OkRes<R> { t: 'ok'; id: number; result: R }
```

### 3.2 Workspace document (Q17)

```ts
export type SessionId = number; export type TabId = number; export type SurfaceId = number;

export interface Workspace {
  revision: number;                 // increments on every change
  active_session: SessionId;
  sessions: Session[];              // ordered
}
export interface Session { id: SessionId; name: string; active_tab: TabId | null; tabs: Tab[] }
export interface Tab { id: TabId; surface: SurfaceId }          // exactly one Surface in v1 (Q19)
export interface SurfaceMeta {
  id: SurfaceId; title: string; user_title: string | null;      // user_title from surface.rename
  cwd: string | null; cols: number; rows: number;
  state: { kind: 'running' } | { kind: 'exited'; code: number | null; signal: string | null };
  view_state: ViewState;
}
export interface ViewState {
  scroll_offset: number;            // lines above the bottom; 0 = following output
  selection: Selection | null;
}
export interface Selection {
  kind: 'normal' | 'block' | 'lines';
  anchor: { line: number; col: number };   // line = absolute line id (§8), so it survives scrolling
  head:   { line: number; col: number };
}
export interface WorkspaceSnapshot { workspace: Workspace; surfaces: SurfaceMeta[] }
```

Mutating requests accept an optional `if_revision`; when present and stale, the server answers `conflict` and the client re-reads. Every successful mutation bumps `revision` and pushes `ev.workspace` to all subscribers (including the requester, which is how edits are "echoed back").

### 3.3 Full v1 request list

```ts
export interface SpawnSpec {
  shell?: string[];                 // argv; default from config.toml
  cwd?: string;                     // default: config / $HOME
  env?: Record<string, string>;     // merged over the server's environment
  cols: number; rows: number;
}

export type Req =
  // workspace
  | { t: 'workspace.get';       id: number }
  | { t: 'workspace.subscribe'; id: number }
  // sessions
  | { t: 'session.create';     id: number; name: string; if_revision?: number }
  | { t: 'session.rename';     id: number; session: SessionId; name: string; if_revision?: number }
  | { t: 'session.delete';     id: number; session: SessionId; if_revision?: number }   // kills its surfaces (Q21)
  | { t: 'session.list';       id: number }
  | { t: 'session.set_active'; id: number; session: SessionId }
  // tabs
  | { t: 'tab.create';         id: number; session: SessionId; index?: number;
      spawn?: SpawnSpec; surface?: SurfaceId; if_revision?: number }   // exactly one of spawn|surface
  | { t: 'tab.close';          id: number; tab: TabId; if_revision?: number }            // kills surface (Q21)
  | { t: 'tab.reorder';        id: number; tab: TabId; index: number; if_revision?: number }
  | { t: 'tab.move';           id: number; tab: TabId; to_session: SessionId; index?: number; if_revision?: number }
  | { t: 'tab.set_active';     id: number; tab: TabId }
  // surfaces
  | { t: 'surface.create';     id: number; spawn: SpawnSpec }        // detached surface; tab.create adopts it
  | { t: 'surface.kill';       id: number; surface: SurfaceId; signal?: 'HUP' | 'TERM' | 'KILL' }
  | { t: 'surface.rename';     id: number; surface: SurfaceId; user_title: string | null }
  // view state (Q17, Q24)
  | { t: 'view.set';           id: number; surface: SurfaceId; scroll_offset?: number; selection?: Selection | null }
  // server
  | { t: 'server.status';      id: number }
  | { t: 'server.shutdown';    id: number; force?: boolean };        // refuses if surfaces exist unless force

export interface ResultMap {
  'workspace.get':       WorkspaceSnapshot;
  'workspace.subscribe': WorkspaceSnapshot;              // initial state, then ev.workspace
  'session.create':      { session: SessionId; revision: number };
  'session.rename':      { revision: number };
  'session.delete':      { revision: number };
  'session.list':        { sessions: Session[] };
  'session.set_active':  { revision: number };
  'tab.create':          { tab: TabId; surface: SurfaceId; revision: number };
  'tab.close':           { revision: number };
  'tab.reorder':         { revision: number };
  'tab.move':            { revision: number };
  'tab.set_active':      { revision: number };
  'surface.create':      { surface: SurfaceId };
  'surface.kill':        {};
  'surface.rename':      { revision: number };
  'view.set':            { revision: number };
  'server.status':       ServerStatus;
  'server.shutdown':     {};
}
export interface ServerStatus {
  build_id: string; proto_version: string; pid: number; uptime_s: number;
  surfaces: number; control_clients: number; data_clients: number;
  workspace_file: string;                                // $XDG_STATE_HOME/superterminal/workspace.json (Q18)
}

export type Ev =
  | { t: 'ev.workspace';      revision: number; workspace: Workspace; surfaces: SurfaceMeta[] } // full doc (small)
  | { t: 'ev.surface_exited'; surface: SurfaceId; code: number | null; signal: string | null }
  | { t: 'ev.server_shutting_down'; reason: string };

export type Res = OkRes<ResultMap[keyof ResultMap]> | ErrRes;
export type ControlMsg = Req | Res | Ev
  | { t: 'hello'; proto_version: string; client_kind: 'control' | 'tool'; build_id: string }
  | { t: 'hello.ack'; proto_version: string; server_build_id: string; workspace_revision: number; server_pid: number }
  | { t: 'reject'; reason: string; message: string; server_version: string };
```

`view.set` is the persistence path for Q24's `SetSelection`; the client sends it debounced (≈50 ms) while dragging and immediately on mouse-up. It bumps `revision` but the server **does not** echo `ev.workspace` for view-only changes to the originating connection (avoids feedback jitter); other subscribers get it.

`ev.workspace` pushes the full document. The document is a few KB for dozens of tabs, so fine-grained patch events are deferred (§12).

### 3.4 Examples

`tab.create` (request and response):

```json
{"t":"tab.create","id":7,"session":1,"spawn":{"cwd":"/home/sonny/projects/superterminal","cols":200,"rows":60},"if_revision":41}
{"t":"ok","id":7,"result":{"tab":12,"surface":9,"revision":42}}
```

`view.set` after a selection drag:

```json
{"t":"view.set","id":8,"surface":9,"selection":{"kind":"normal","anchor":{"line":10342,"col":0},"head":{"line":10343,"col":17}}}
{"t":"ok","id":8,"result":{"revision":43}}
```

Error, then event:

```json
{"t":"tab.close","id":9,"tab":999}
{"t":"err","id":9,"error":{"code":"not_found","message":"tab 999 does not exist"}}
{"t":"ev.surface_exited","surface":9,"code":0,"signal":null}
```

---

## 4. DATA messages (binary)

### 4.1 Type table

| `msg_type` | Direction | Struct | | `msg_type` | Direction | Struct |
|---|---|---|---|---|---|---|
| `0x0001` | C→S | `Hello` | | `0x0100` | S→C | `Snapshot` |
| `0x0002` | S→C | `HelloAck` | | `0x0101` | S→C | `Delta` |
| `0x0003` | S→C | `Reject` | | `0x0102` | S→C | `History` |
| `0x0010` | C→S | `Attach` | | `0x0103` | S→C | `SurfaceExited` |
| `0x0011` | C→S | `Detach` | | `0x0104` | S→C | `ModeChanged` |
| `0x0012` | C→S | `Input` | | `0x0105` | S→C | `TitleChanged` |
| `0x0013` | C→S | `Resize` | | `0x0106` | S→C | `Bell` |
| `0x0014` | C→S | `FetchHistory` | | `0x0107` | S→C | `Detached` |
| `0x0015` | C→S | `Ack` | | `0x01FF` | S→C | `DataError` |

Range `0x0000–0x00FF` is client→server (plus handshake), `0x0100–0x01FF` server→client. Future minors allocate upward.

### 4.2 Client → server

```rust
pub type SurfaceId = u32;
pub type Seq = u64;        // per-Surface, monotonic, starts at 1 on Surface creation
pub type LineId = u64;     // absolute line id since Surface creation (§8)

#[derive(Serialize, Deserialize)]
pub struct Attach {
    pub surface_id: SurfaceId,
    pub want_snapshot: bool,      // true: always send Snapshot. false: only if known_seq is stale
    pub known_seq: Seq,           // 0 = nothing known
}

#[derive(Serialize, Deserialize)] pub struct Detach { pub surface_id: SurfaceId }

#[derive(Serialize, Deserialize)]
pub struct Input { pub surface_id: SurfaceId, pub bytes: Vec<u8> }   // ≤ 64 KiB per frame; client chunks pastes

#[derive(Serialize, Deserialize)]
pub struct Resize { pub surface_id: SurfaceId, pub cols: u16, pub rows: u16 }

#[derive(Serialize, Deserialize)]
pub struct FetchHistory { pub surface_id: SurfaceId, pub from_line: LineId, pub count: u16 }  // count ≤ 1000 (Q25)

#[derive(Serialize, Deserialize)]
pub struct Ack { pub surface_id: SurfaceId, pub seq: Seq }          // "I have applied everything ≤ seq"
```

### 4.3 Server → client

```rust
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub surface_id: SurfaceId,
    pub seq: Seq,
    pub cols: u16, pub rows: u16,
    pub styles: Vec<Style>,             // full style table; index = position (0 = default)
    pub grid: Vec<Row>,                 // exactly `rows` entries, top to bottom
    pub cursor: Cursor,
    pub modes: Modes,
    pub title: String,
    pub history_base: LineId,           // id of the oldest retained history line
    pub history_len: u32,               // retained history lines (not their content)
    pub view_state: ViewState,          // mirrors the CONTROL-plane value at this seq
    pub exited: Option<ExitStatus>,
}

#[derive(Serialize, Deserialize)]
pub struct Delta {
    pub surface_id: SurfaceId,
    pub seq: Seq,
    pub scrollback_appended: u32,       // lines pushed off the top into history since prev seq
    pub history_base: LineId,           // post-update; lets the client track trimming
    pub resized: Option<(u16, u16)>,    // (cols, rows) when the grid size changed in this delta
    pub new_styles: Vec<(u16, Style)>,  // table additions/replacements, applied before rows
    pub rows: Vec<DirtyRow>,            // each is FULL row content (Q16)
    pub cursor: Cursor,
    pub modes: Modes,
    pub title: Option<String>,          // Some only when changed
}

#[derive(Serialize, Deserialize)] pub struct DirtyRow { pub index: u16, pub row: Row }

#[derive(Serialize, Deserialize)]
pub struct History {
    pub surface_id: SurfaceId,
    pub from_line: LineId,              // first line actually returned (≥ requested; see §8)
    pub history_base: LineId,           // current trim point
    pub rows: Vec<Row>,
}

#[derive(Serialize, Deserialize)] pub struct ExitStatus { pub code: Option<i32>, pub signal: Option<i32> }
#[derive(Serialize, Deserialize)] pub struct SurfaceExited { pub surface_id: SurfaceId, pub seq: Seq, pub status: ExitStatus }
#[derive(Serialize, Deserialize)] pub struct ModeChanged  { pub surface_id: SurfaceId, pub seq: Seq, pub modes: Modes, pub cursor: Cursor }
#[derive(Serialize, Deserialize)] pub struct TitleChanged { pub surface_id: SurfaceId, pub seq: Seq, pub title: String }
#[derive(Serialize, Deserialize)] pub struct Bell         { pub surface_id: SurfaceId }
#[derive(Serialize, Deserialize)] pub struct Detached     { pub surface_id: SurfaceId, pub reason: DetachReason }
#[derive(Serialize, Deserialize)] pub enum   DetachReason { Requested, SurfaceDestroyed, ServerShutdown }
#[derive(Serialize, Deserialize)] pub struct DataError    { pub surface_id: Option<SurfaceId>, pub code: u16, pub message: String }
```

`ModeChanged`, `TitleChanged`, `SurfaceExited` carry a `seq`: they consume a sequence number exactly like a Delta so the gap detector (§6) sees one totally ordered stream per Surface. They are sent *instead of* an otherwise empty Delta when a mode/title/exit change occurs with no dirty rows in the coalescing window; when grid damage coincides, the change rides inside the Delta (`modes`, `title`) and no standalone message is sent. Clients treat both paths identically. `Bell` and `Detached` are outside the sequence.

### 4.4 Shared types

```rust
#[derive(Serialize, Deserialize)]
pub struct Row { pub cells: Vec<PackedCell>, pub extras: Vec<String>, pub wrapped: bool }
// `cells.len() ≤ cols`; trailing cells equal to PackedCell::BLANK are omitted and the client pads.
// `wrapped`: this row soft-wraps into the next (copy/paste joins them without '\n').

#[derive(Serialize, Deserialize)]
pub struct Cursor { pub row: u16, pub col: u16, pub shape: CursorShape, pub visible: bool, pub blink: bool }
#[derive(Serialize, Deserialize)] pub enum CursorShape { Block, Underline, Beam }

bitflags! { #[derive(Serialize, Deserialize)] pub struct Modes: u16 {
    const ALT_SCREEN      = 1 << 0;
    const BRACKETED_PASTE = 1 << 1;
    const MOUSE_CLICK     = 1 << 2;   // 1000
    const MOUSE_DRAG      = 1 << 3;   // 1002
    const MOUSE_MOTION    = 1 << 4;   // 1003
    const MOUSE_SGR       = 1 << 5;   // 1006 encoding
    const APP_CURSOR_KEYS = 1 << 6;   // DECCKM
    const APP_KEYPAD      = 1 << 7;
    const FOCUS_EVENTS    = 1 << 8;   // 1004
    const LINE_WRAP       = 1 << 9;   // DECAWM
    const KITTY_KEYBOARD  = 1 << 10;  // reserved; not emitted in 1.0
}}
```

---

## 5. Cell and style encoding (Q16)

### 5.1 Packed cell

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub struct PackedCell {
    pub codepoint: u32,   // Unicode scalar; or index into Row.extras when GRAPHEME_EXT set
    pub style_idx: u16,   // into the Surface's StyleTable; 0 = default style
    pub flags: u8,        // CellFlags below
}
impl PackedCell { pub const BLANK: Self = Self { codepoint: 0x20, style_idx: 0, flags: 0 }; }

bitflags! { pub struct CellFlags: u8 {
    const WIDE                = 1 << 0;  // leading cell of a 2-column glyph
    const WIDE_SPACER         = 1 << 1;  // trailing half; codepoint = 0, not rendered
    const GRAPHEME_EXT        = 1 << 2;  // codepoint field is an index into Row.extras
    const WIDE_LEADING_SPACER = 1 << 3;  // filler at row end when a wide glyph wrapped to next row
    // bits 4–7 reserved (must be 0 in 1.x)
}}
```

In memory the replica stores this as 8 bytes (`u32 + u16 + u8 + 1 pad`), giving a 200×60 grid of 96 KB — trivially cache-friendly. On the wire postcard varint-encodes the integers: an ASCII cell with a small style index is **3 bytes**, a CJK/emoji codepoint with `style_idx ≥ 128` is at most 6.

### 5.2 Grapheme clusters wider than one codepoint

Most cells are one scalar. When a cell holds a multi-codepoint cluster (base + combining marks, ZWJ emoji sequences, variation selectors, regional-indicator pairs), the server sets `GRAPHEME_EXT`, appends the full cluster (as a UTF-8 `String`) to that row's `extras` list, and stores the extras **index** in `codepoint`. The overflow table is **per row**, rebuilt whenever the row is sent, so indices never dangle: a Delta carrying a row always carries that row's complete `extras`. Width (1 or 2 columns) is still expressed by `WIDE`/`WIDE_SPACER`, so layout never needs to inspect `extras`. The client's shaping cache keys on the cluster string, not the index.

### 5.3 Style table

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color { Default, Indexed(u8), Rgb(u8, u8, u8) }

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style { pub fg: Color, pub bg: Color, pub underline_color: Color, pub attrs: Attrs }

bitflags! { #[derive(Serialize, Deserialize, Hash)] pub struct Attrs: u16 {
    const BOLD          = 1 << 0;
    const DIM           = 1 << 1;
    const ITALIC        = 1 << 2;
    const UNDERLINE     = 1 << 3;   // kind in bits 4–6 (0 = single when UNDERLINE set)
    const UL_DOUBLE     = 1 << 4;
    const UL_CURLY      = 2 << 4;
    const UL_DOTTED     = 3 << 4;
    const UL_DASHED     = 4 << 4;
    const STRIKETHROUGH = 1 << 7;
    const INVERSE       = 1 << 8;
    const HIDDEN        = 1 << 9;
    const BLINK         = 1 << 10;
}}
```

Interning: the server keeps one `HashMap<Style, u16>` per Surface. Index 0 is always the all-`Default`, `attrs = 0` style. New styles are assigned the next free index and shipped in `Delta.new_styles` **before** the rows that reference them; a `Snapshot` carries the whole table. The client mirrors the table verbatim. Entries are never freed within a Surface's lifetime in 1.0; if the table reaches 65 535 entries the server compacts (re-numbers by liveness on the visible grid + retained history) and forces a `Snapshot`. Since 256 indexed × 256 indexed × few attrs dominates real programs, tables stay in the low hundreds.

---

## 6. Delta semantics

1. **Sequence.** `seq` is per Surface, starts at 1 (the Surface's creation state), and increments by exactly 1 for every `Snapshot`, `Delta`, `ModeChanged`, `TitleChanged`, or `SurfaceExited` the *authoritative state* produces — not per client. All attached clients see the same `seq` values; a client that was coalesced (below) simply skips numbers, which is legal because a coalesced Delta is tagged with the *latest* seq it folds in and carries `prev_seq`-free semantics: **a Delta's content is the full dirty set relative to whatever the client last acknowledged** (the server tracks per-client `last_sent_seq` and accumulates dirty rows since then).
2. **Applying a Delta** in order: `new_styles` → if `resized`, resize the replica (rows cleared, then all rows arrive dirty) → shift the visible grid up by `scrollback_appended`, moving the top rows into the local history cache with ids `first_visible_id … +n-1`, then `first_visible_id += n` → replace each `DirtyRow.index` with the given row (pad to `cols`) → set `cursor`, `modes`, `title` (if `Some`), `history_base`. If `scrollback_appended > rows`, the client's history cache has a hole for the lines it never saw; it fetches them on demand (§8).
3. **Gap detection.** The client keeps `last_seq` per attached Surface. Because the server folds skipped states into the next Delta, the client cannot infer a gap from `seq` alone; instead the server stamps every Delta with `since_seq` — *addendum to `Delta` (see §12, gap 1)*: `pub since_seq: Seq` — and the client checks `since_seq == last_seq`. On mismatch, or on any decode error, the client sends `Attach{want_snapshot: true, known_seq: last_seq}` and discards Deltas until the `Snapshot` arrives.
4. **Coalescing (Q27).** Per client, per Surface, the server emits at most one Delta per **1/120 s**. Damage inside the window is merged (dirty-row set union, `scrollback_appended` summed, styles appended, latest cursor/modes/title). When the PTY goes quiet the server **always flushes the final state** within one window, so the client never displays a stale frame.
5. **Flow control.** The server allows at most **4 unacknowledged** Deltas in flight per (client, Surface). `Ack{seq}` from the client (sent after applying, at most once per rendered frame) reopens the window. While the window is closed the server keeps merging into a single pending Delta — nothing is dropped, memory is bounded by one grid — and sends it on the next `Ack`. A `Snapshot` counts as one in-flight message. Ack is per Surface so a busy `cat` in one tab never starves the cursor blink in another.
6. **Alt-screen.** Switching to/from alt screen sets `ALT_SCREEN` and marks all rows dirty; `scrollback_appended` is always 0 while `ALT_SCREEN` is set (alt screen has no scrollback).

---

## 7. Snapshot contents

A `Snapshot` (§4.3) is complete: on receipt the client **discards** the replica's visible grid, style table, cursor, modes, title and `view_state` and replaces them. It carries `history_base` and `history_len` but **no history content**; the client's history cache is kept if its line ids are ≥ `history_base` (they are still valid) and otherwise dropped. Snapshots are sent on `Attach` (when `want_snapshot` or `known_seq ≠ current seq`), after style-table compaction, and after a `Resize` when the engine reflows (see §12). The client's scrollbar uses `history_len + rows` immediately, before any history is fetched (Q25).

---

## 8. History paging

**Line ids.** Every line the Surface has ever produced, including the current visible rows, has a `LineId` assigned once at creation and never renumbered. Ids increase downward. Given a `Snapshot`/`Delta`:

```
first_visible_id = history_base + history_len          (visible row r has id first_visible_id + r)
oldest_available = history_base
```

When scrollback exceeds the cap, the server trims from the oldest end and advances `history_base`; ids of everything else are unchanged, so selections and cached rows stay valid without any client bookkeeping. `Delta.history_base` lets the client drop cached rows below it.

**Cap.** `scrollback_lines` in `config.toml`, default **10 000**, per Surface, applied on the server; the client's cache may be smaller (LRU) and is refilled by fetching.

**FetchHistory.** `FetchHistory{surface_id, from_line, count ≤ 1000}` asks for lines `[from_line, from_line + count)` intersected with `[history_base, first_visible_id)`. The response `History{from_line', history_base, rows}` starts at `from_line' = max(from_line, history_base)`; `rows` may be shorter than `count` at either end; an empty `rows` with `history_base > from_line + count` tells the client those lines are gone. Requests are answered in order per connection; the client issues at most 2 outstanding pages per Surface and prefetches one page beyond the viewport in the scroll direction (Q25). History rows use the same `Row` encoding and the same style table as the grid.

---

## 9. Input encoding

- **Keys are encoded on the client, in Rust** (Q23). `<terminal-grid>` converts GPUI key events to VT byte sequences honoring the Surface's current `Modes` (`APP_CURSOR_KEYS`, `APP_KEYPAD`; `KITTY_KEYBOARD` reserved) and sends `Input{surface_id, bytes}`. The server writes bytes to the PTY verbatim, no interpretation, no echo. It also feeds them to nothing else — local echo is the program's job.
- **Mouse** (Q24): when `modes` has any `MOUSE_*` bit and Shift is not held, the element encodes X10/SGR reports client-side into `Input`. Focus in/out (`\e[I`/`\e[O`) likewise when `FOCUS_EVENTS` is set.
- **Paste bracketing happens on the client.** If `modes ∩ BRACKETED_PASTE`, the client sends `\e[200~` + text + `\e[201~`, otherwise text alone; in both cases `\n` is normalized to `\r` unless bracketed. Text is chunked into ≤ 64 KiB `Input` frames; bracketing markers wrap the whole paste, not each chunk. The server never inspects `Input` for bracketing — it cannot know the client's intent, and the client already has the mode from the last Delta.
- **Resize**: `Resize{cols, rows}` → server resizes the PTY (`TIOCSWINSZ`) and the engine; the resulting damage arrives as a Delta with `resized: Some(..)` and all rows dirty. With several clients attached, the last `Resize` wins (§12).
- **Exited Surface** (Q22): `Input` to an exited Surface is dropped with `DataError{code: 0x0001 "surface_exited"}`; the client handles "press Enter to close" locally via CONTROL `tab.close`.

---

## 10. Versioning and compatibility

- `ProtoVersion{major, minor}`; 1.0 is this document. Negotiated minor = `min`. Both sides must behave exactly as that minor specifies.
- **CONTROL (JSON, self-describing):** adding an *optional* field to any message, adding a new request/event, or adding a new `ErrorCode` is **minor**. Receivers ignore unknown fields and unknown `ev.*`; unknown request `t` → `err{unsupported}`. Renaming/removing/retyping a field, or changing a field from optional to required, is **major**.
- **DATA (postcard, positional):** adding a field to an *existing* struct is **not** possible within a major — the encoder must emit the layout of the negotiated minor, so in practice a new field means a **new `msg_type`** (e.g. `Delta2 = 0x0108`) that the server sends only when negotiated minor ≥ N; the old type stays for older clients. Adding a new `msg_type`, a new enum variant *at the end* of an enum that is only ever sent by the newer side under negotiation, or new bit positions in `Modes`/`Attrs`/`CellFlags` (older receivers mask reserved bits) is **minor**. Reordering fields, changing integer widths, changing `PackedCell`, or changing the frame header is **major**.
- The generated TS types (`packages/protocol-ts`) and Rust types (`st-proto`) are produced from the Rust definitions (e.g. `ts-rs` or `specta`); a round-trip test fixture directory `crates/st-proto/fixtures/v1.*/` holds golden encodings and every minor must still decode every older fixture (Q33).

| Change | Plane | Class | Why |
|---|---|---|---|
| Add `Tab.color?: string` to the Workspace document | CONTROL | **minor** | JSON is self-describing; old clients ignore it, old servers never send it |
| Add `SearchHistory{surface_id, query}` / `SearchResult` messages | DATA + CONTROL | **minor** | New `msg_type` 0x0016/0x0109 sent only when negotiated minor ≥ 1.1; unknown types are never emitted to older peers |
| Change `PackedCell` to `u32 codepoint \| u8 style_idx \| u8 flags` or add a fourth field | DATA | **major** | Positional postcard layout of the hottest struct changes; every Row in every message is affected |

---

## 11. Size and performance budget

Assumptions: postcard varints; ASCII text with `style_idx < 128` = 3 B/cell; blank tails trimmed; frame header 6 B.

**Full 200×60 Snapshot.**
- Dense text (every cell non-blank, e.g. `btop`): 12 000 cells × 3 B ≈ **36 KB** + rows overhead (60 × ~4 B) + style table (200 entries × ≤14 B ≈ 2.8 KB) + cursor/modes/title (~60 B) → **≈ 39 KB**.
- Worst case (all CJK/emoji, `style_idx ≥ 128`, one `extras` cluster per cell): 6 B/cell + ~12 B/cluster string → ≈ **220 KB**. Still 36× under the 8 MiB frame cap.
- Typical shell screen (≈40 % of cells non-blank): **≈ 15 KB**. At Unix-socket throughput (≥ 1 GB/s) all of these are < 0.3 ms — attach-to-first-paint is dominated by the PTY snapshot and shaping, not the wire (Q27 target < 100 ms).
- Replica memory: 200×60 × 8 B = 96 KB visible + 10 000 × 200 × 8 B = 16 MB of history if fully cached (the client LRU caps this at ~2 000 cached lines ≈ 3.2 MB).

**Typical Delta: one line of `ls` output** (a new 40-char line scrolls the screen by one).
- With the `scrollback_appended` shift (§6.2) the engine reports one dirty row (the new bottom line) plus the row where the cursor/prompt was: 2 × (~40 × 3 B + 4) ≈ 250 B + header ≈ **≈ 270 B**.
- If the engine's damage tracking marks the whole screen on scroll (see §12), all 60 rows are dirty: 60 × 80 × 3 B ≈ **14 KB** on an 80-wide, up to 36 KB on a dense 200-wide grid. Either way it is one frame at 120 Hz.

**Worst case: `cat` of a 1 GB file at 120 Hz**, 200×60, dense.
- Every Delta: all 60 rows dirty ≈ 36 KB; `scrollback_appended` in the thousands but *no history content is pushed*, so the size is bounded by the visible grid, not by throughput of the program.
- 120 Hz × 36 KB ≈ **4.3 MB/s** per attached client — under 0.5 % of a Unix socket's bandwidth, and decoding 12 000 cells per frame is ≈ 50–100 µs on the client. The Ack window (4 in flight) means a client that renders at 60 Hz receives ≈ 60 coalesced Deltas/s instead, halving the bytes with no loss of final state. The server's own cost is dominated by the VT parser consuming the PTY output, which the protocol does not change.
- Style-table churn is negligible: `cat` of plain text adds zero styles; a colorful `rg` adds a handful per Delta at 6–14 B each.

---

## 12. Open questions

1. **`since_seq` on Delta.** §6.3 needs the client to detect gaps although the server legitimately skips seq numbers for coalesced clients. Proposed: add `since_seq: Seq` to `Delta` (and to `ModeChanged`/`TitleChanged`/`SurfaceExited`) so the client checks `since_seq == last_seq`. Alternative: per-client seq numbering (simpler check, but different clients then see different seqs for the same state, which complicates `known_seq` on re-Attach). Needs a decision before `st-proto` is written; the field is included in the 1.0 fixtures either way.
2. **Standalone `ModeChanged`/`TitleChanged` vs. always-Delta.** Q16 puts modes and title *inside* the Delta; the message list here also has standalone events. §4.3 defines both with a folding rule. Simplification candidate: drop the standalone messages and send an empty Delta (`rows: []`) — one code path, ~10 extra bytes per event.
3. **Scroll damage granularity.** The 270 B `ls` estimate depends on the VT engine reporting per-line damage after a scroll rather than full-screen damage. `alacritty_terminal`'s damage tracker is believed to mark the whole viewport on scroll; if so, the server needs to compute the shift itself (compare the line-id ring before/after) to emit only truly changed rows. The protocol is unaffected; the server budget in `03-server.md` should account for it.
4. **Resize with several clients attached at different sizes.** §9 says last `Resize` wins. tmux uses the smallest attached size and letterboxes; Superlogical's behavior is unknown. Also: does the engine **reflow** history on width change (alacritty does)? Reflow invalidates the "line ids are never renumbered" invariant of §8 for rewrapped lines. Options: disable reflow in v1 (simplest, keeps ids stable), or bump `history_base` to the reflow boundary and force a Snapshot.
5. **Trailing-blank trimming** (§4.4) is an encoding optimization over Q16's "full row content"; it changes the wire size estimates materially. Confirm it is acceptable (semantically the row is still complete).
6. **Row `wrapped` flag** is not in Q16 but is needed for correct copy/paste of soft-wrapped lines; it adds 1 byte per row. Include in 1.0?
7. **`tab.set_active`** and `Session.active_tab` are not in the grilling document, which only mentions switching the active Session. The client needs the active tab persisted for "relaunch looks identical". Assumed in; confirm.
8. **Workspace change events.** Full-document `ev.workspace` on every change is simple but chatty during rapid `view.set` (scroll wheel). Currently mitigated by suppressing echo to the originator; if a second client is attached it receives one document per scroll tick. Fine-grained patch events (`ev.view_changed`) would be a minor addition later.
9. **`Input` to an exited Surface** returns `DataError`; alternatively the server could silently drop. Decide whether `DataError` should be per-message or connection-fatal.
10. **Style-table compaction** at 65 535 entries is specified but essentially untested territory; an alternative is a smaller fixed cap (e.g. 4 096) with LRU eviction and `Snapshot` on overflow, which would also let `style_idx` be a `u8`+escape in a future major.
11. **Authentication.** Same-user Unix socket permissions are the only auth in v1. If the SSH hook (Q5) is ever used, `Hello` needs a token field — a minor CONTROL change but a new `Hello2` type on DATA; consider reserving it now.
