---
status: accepted
---
# Authoritative terminal state in the Server, Replica grids in the Client, synced by Snapshot + Deltas

Instead of streaming raw PTY bytes to the Client (tmux/ssh style, which forces the Client to re‑parse and re‑build scrollback on every reconnect), the Server runs the VT state machine and publishes row‑granular Deltas with a sequence number; each Client keeps a Replica it paints from. Reconnect is one Snapshot, not a replay; scrolling and selection are local and instant. This is the same "N replica terminal state machines" idea Superlogical describes.

## Considered options
- Raw byte stream + client‑side parser — rejected: slow reattach, every client re‑parses, mouse/selection state can't be shared.
- Server renders pixels/glyph lists — rejected: ties Server to fonts and DPI, huge bandwidth.

## Consequences
- The Client needs its own grid model (the Replica) that can be driven by Deltas; it cannot reuse `alacritty_terminal::Term` (ADR‑0004).
- A protocol version must be negotiated; cell packing changes are breaking (see 02‑protocol).
