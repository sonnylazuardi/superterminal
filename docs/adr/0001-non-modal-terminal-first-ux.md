---
status: accepted
---
# Non‑modal, "just a terminal" UX — no prefix key, no status bar

The product's reason to exist is persistence and multiplexing that feel like an ordinary local terminal (the Superlogical demo's central argument). We decided that all multiplexer features are exposed only through native GUI affordances (tabs, palette, ordinary app shortcuts) and never through a tmux‑style prefix key, status line or modal layer. This is hard to reverse because it shapes every keyboard‑routing decision between the native grid element and the React chrome (see ADR‑0005), and a future reader comparing us to tmux/zellij will otherwise wonder where the "mode" went.

## Consequences
- Every shortcut must be one a terminal program would not need; conflicts are resolved in favour of the terminal (Ctrl+Shift+… on Linux, ⌘ on macOS).
- No feature may require the user to learn a mode; if it would, it is out of scope.
