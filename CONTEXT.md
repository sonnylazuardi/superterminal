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
An ordered slot inside a Session shown in the tab strip. It holds one or more Panes arranged by Splits; a fresh Tab is a single Pane.
_Avoid_: window, pane (a Pane is a part of a Tab, not a Tab)

**Pane**:
One rectangular region of a Tab showing exactly one Surface. Every Tab has at least one; the Pane that has the keyboard is the focused Pane.
_Avoid_: split (that is the act), panel, view, cell

**Split**:
Dividing a Pane in two, seeding the new Pane with a fresh Surface. The two Splits are named by where the new Pane goes: **Split Right** puts it beside the original, **Split Down** puts it below. A Tab's layout is a tree of Splits whose leaves are Panes; closing a Pane collapses its Split into the surviving sibling. Splits belong to the Workspace and survive the Client closing.
_Avoid_: pane (the result), tile, frame, vertical/horizontal split (they mean opposite things in iTerm and tmux)

**Surface**:
One running program attached to a pseudo‑terminal together with its authoritative terminal state (screen, scrollback, cursor, modes, title). Surfaces belong to the Server.
_Avoid_: terminal, pty, buffer, shell, pane (a Pane shows a Surface; it is not one)

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
The per‑Surface, user‑visible position and selection (scroll offset, selected range) that survives Client relaunch. Owned by the Server.
_Avoid_: viewport, UI state

**Config**:
What the user has declared they want, written by hand in the configuration file (font, shell, keybindings, initial window size, initial tab layout). Read at start‑up; never written by the program.
_Avoid_: settings, preferences (both blur the line with Client State)

**Client State**:
What one Client on one machine remembers from its last run without the user declaring it: the last window size, the Tab Layout (sidebar or strip) and the sidebar width. Client State wins over Config when both say something about the same thing; Config only seeds the first run. It is not part of the Workspace and is never sent to the Server.
_Avoid_: settings, preferences, window state, ui state, cache

**Control Plane**:
The human‑readable channel between Client and Server for managing the Workspace (create Tab, rename Session, …).
_Avoid_: API, RPC, management channel

**Data Plane**:
The compact channel between Client and Server carrying Snapshots, Deltas, History and input bytes.
_Avoid_: stream, hot path (in prose)

**Tab Layout**:
Where the tab strip lives: a **sidebar** down the left edge (the default) or a **strip** along the top. Toggled at runtime; remembered as Client State.
_Avoid_: vertical/horizontal tabs (acceptable in UI copy and code identifiers only), orientation

**Dialog**:
A floating panel that takes the keyboard while open (the Command Palette, the Session switcher). Every Dialog opens at the top centre of the window.
_Avoid_: modal (there is no modal state; Esc always dismisses), popup, overlay (the paint layer, not the concept)

**Menu**:
A floating list of Commands opened at the pointer by a right‑click (e.g. on a Tab). Like a Dialog it takes the keyboard while open — arrows move, Enter runs, Esc or a click elsewhere closes — but it opens where it was invoked, not at the top centre.
_Avoid_: context menu (acceptable in UI copy only), popup, dropdown

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
