---
status: accepted
---
# `alacritty_terminal` as the Server's VT engine, behind a `VtEngine` trait

We need a correct VT parser and grid with damage tracking today. We chose the `alacritty_terminal` crate (Rust, mature, used by Zed) for the Server's authoritative state machine rather than writing our own or binding libghostty‑vt (C ABI, requires a Zig toolchain). It is wrapped in a small `VtEngine` trait (advance bytes, take damage, snapshot, resize, history range) so the engine can be swapped without touching the protocol or Client.

## Consequences
- Our cell/style model is a projection of alacritty's, not alacritty's types on the wire.
- Features alacritty lacks (e.g. Kitty graphics) wait for an engine swap or upstream work.
