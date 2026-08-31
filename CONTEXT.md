# Superterminal

A GPU‑rendered terminal client whose terminals live in a persistent local server, so processes survive the window closing and the client reconnects instantly. This glossary is the ubiquitous language for the whole repo; it deliberately contains no implementation detail.

## Language

### Processes

**Server**:
The per‑user background process that owns every running terminal and all Workspace state. It has no window.
_Avoid_: daemon (in prose), backend, multiplexer (as a noun for the process)

**Client**:
A process with a window that displays Surfaces from one Server and sends user input to them. Closing a Client never stops anything in the Server.
_Avoid_: frontend, GUI, app (in domain prose)

### Workspace structure

**Workspace**:
Everything one user has on one Server: all Sessions, Tabs and Surfaces, plus which Session is active.
_Avoid_: state, world

**Session**:
A named, ordered group of Tabs (e.g. "Demo", "Work"). Exactly one Session is active in a Client at a time.
_Avoid_: workspace (for this level), project, group

**Tab**:
An ordered slot inside a Session that holds exactly one Surface and is shown in the tab strip.
_Avoid_: window, pane

**Surface**:
One running program attached to a pseudo‑terminal together with its authoritative terminal state (screen, scrollback, cursor, modes, title). Surfaces belong to the Server.
_Avoid_: terminal, pty, buffer, shell, pane

### Replication

**Replica**:
A Client's local copy of one Surface's screen, scrollback and modes, kept current by Deltas so the Client can scroll, select and paint without asking the Server.
_Avoid_: cache, mirror, view model

**Attach**:
The act of a Client subscribing to a Surface so it receives a Snapshot followed by Deltas. Detach is the reverse and does not affect the Surface.
_Avoid_: connect (reserved for sockets), subscribe, open

**Snapshot**:
The complete current state of a Surface's screen sent on Attach or when a Client has fallen behind.
_Avoid_: full sync, dump, frame

**Delta**:
A sequence‑numbered description of what changed in a Surface since the previous Delta (dirty rows, cursor, modes, appended scrollback).
_Avoid_: diff, patch, update, damage (Server‑internal term for what the terminal engine reports)

**History**:
The lines that have scrolled off the top of a Surface's screen. Fetched by a Client on demand, not pushed.
_Avoid_: scrollback (acceptable in UI copy only), backlog

### Interaction

**View State**:
The per‑Surface, user‑visible position and selection (scroll offset, selected range) that survives Client relaunch.
_Avoid_: viewport, UI state

**Control Plane**:
The human‑readable channel between Client and Server for managing the Workspace (create Tab, rename Session, …).
_Avoid_: API, RPC, management channel

**Data Plane**:
The compact channel between Client and Server carrying Snapshots, Deltas, History and input bytes.
_Avoid_: stream, hot path (in prose)

**Command**:
A named, user‑invocable action with an optional shortcut, shown in the Command Palette (e.g. "New Tab").
_Avoid_: action, binding (the shortcut is the binding; the Command is what it runs)

**Exited**:
The state of a Surface whose program has terminated; its screen stays readable until the user closes the Tab.
_Avoid_: dead, finished, closed (closing is the user's act)

**Pristine**:
A Surface that is the automatically seeded shell of a Session and has never received input nor started a child process. Pristine Surfaces do not count as work worth keeping a Server alive for.
_Avoid_: empty, idle, unused

**Active / Passive Attach**:
An Active Attach receives the Surface's rows; a Passive Attach receives only title, status, bell and History length. The visible Tab is Active, the other Tabs of the active Session are Passive.
_Avoid_: subscribed, background attach, hot/cold
