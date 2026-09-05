---
status: accepted
---
# Splits live in the Workspace, added to the protocol without a major bump

A Tab holds a tree of Splits whose leaves are Panes, each showing one Surface. The tree is owned by the Server as part of the Workspace (`Tab.layout`), persisted in `workspace.json`, and reaches Clients through the existing Workspace snapshot. The wire change is additive: `Tab.surface` stays (always the first leaf) and `layout` is added beside it, with new `tab.split` / `pane.close` requests; the protocol version moves 1.0 → 1.1.

## Considered options
- **Client-owned splits** (second Surface via `surface.create`, layout in Client State) — rejected: the split would vanish when the window closes and its extra Surface would be orphaned, contradicting the one promise the persistent Server makes; `st ls` and a second Client could not see it.
- **Replace `Tab.surface` with the tree (2.0)** — rejected: a major bump refuses every 1.x client and forces a `workspace.json` migration and a CLI change, for no user‑visible gain. Keeping `surface` as the first leaf lets every existing reader keep working and older daemons still load new files (unknown `layout` is ignored).
- **Server‑owned focused Pane** — rejected: focus is a property of one window's keyboard; two Clients on one Server would fight over it, and it costs a round‑trip per click. Focus is Client UI state; the first leaf is focused on relaunch.

## Consequences
- A 1.0 client connected to a 1.1 daemon shows only the first Pane of a split Tab; that is acceptable degradation, not an error.
- New Splits start at `ratio` 0.5; the divider is draggable, with the ratio applied locally while dragging and sent once on release (`tab.set_ratio`), so a drag costs the grids at most two re‑flows rather than one per pointer move.
- Every Pane of the visible Tab is an Active attach; the LRU of warm Replicas now has to hold more than one Surface per Tab.
- Split names are **Split Right** / **Split Down** (`axis: "row" | "column"`); "vertical/horizontal" never appear, because iTerm and tmux use them with opposite meanings.
