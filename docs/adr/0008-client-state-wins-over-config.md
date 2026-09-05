---
status: accepted
---
# Client State wins over Config for window size and Tab Layout

The Client remembers its last window size and Tab Layout in a small per‑machine file (Client State). `config.toml` can also name a window size and a Tab Layout. When both exist, **Client State wins**; Config only seeds the very first run (or a run after the file was deleted), and `[window] remember = false` turns the memory off entirely.

## Considered options
- **Config wins, Client State fills the gaps** — rejected: the common case is a user who resized the window or toggled the sidebar with the mouse and expects that to stick; a config key they wrote once would silently undo it on every launch. Editing config to change the layout would then require also deleting the state file.
- **Write the toggle back into `config.toml`** — rejected: Config is the user's hand‑written declaration and the program never rewrites it (comments, formatting and the shared Rust reader would all suffer).
- **Store window geometry in the Server's Workspace** — rejected: the Server has no window (CONTEXT.md); geometry is a property of one machine's Client, and a Windows Client and a WSL Client of the same Server should not fight over it.

## Consequences
- Editing `window.width`/`window.height`/`window.vertical_tabs` in Config has no visible effect once Client State exists; the config example says so, and `remember = false` restores config‑driven behaviour.
- Only what gpuix can observe is remembered: the paintable size in logical pixels and the layout. Window position and maximised state are not exposed by the renderer and are not remembered.
- This mirrors Ghostty's `window-save-state` and the macOS convention, so it should not surprise users of other terminals.
