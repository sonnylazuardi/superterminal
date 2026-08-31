---
status: accepted
---
# Two socket connections per Client: JSON Control Plane from Bun, binary Data Plane from Rust

The Client is two runtimes (Bun/JS for chrome, Rust for the grid). Rather than one Rust‑owned multiplexed connection bridged to JS through napi events, each runtime opens its own connection to the Server: Bun speaks newline‑delimited JSON for Workspace management (debuggable with `socat`), the Rust native module speaks length‑prefixed `postcard` frames for Snapshots/Deltas/input. Ordering is safe by construction: JS creates a Surface on the Control Plane and only then renders `<terminal-grid surfaceId>`, whose Attach travels on the Data Plane.

## Considered options
- Single connection owned by Rust, control messages bridged to JS — kept as the documented fallback if cross‑connection races appear; costs a napi request/response layer we don't otherwise need.
- Everything JSON — rejected: Deltas are the hot path.

## Consequences
- The Server treats the two connections as independent clients that happen to share a user; no per‑Client identity spans them in v1.
- Authentication is filesystem permissions on the socket directory.
