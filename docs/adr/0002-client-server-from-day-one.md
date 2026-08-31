---
status: accepted
---
# Client/server split from day one, Server on a Unix domain socket

Terminals must survive the window closing and reappear in well under a second. We decided the Server (`superterminald`) owns every PTY and terminal state machine from the first commit, and the Client is a thin renderer; there is no "single‑process mode". Retrofitting a server later would rewrite the ownership model of the terminal core, so this is effectively irreversible.

## Considered options
- Single process with in‑process persistence (tabs die with the window) — rejected: defeats the product.
- Optional server (attach if present, else local) — rejected: two code paths for the hot path, and the demo's reconnect speed proves a permanent server costs nothing perceptible.

## Consequences
- Every feature is designed as "state in Server, projection in Client" (see CONTEXT.md).
- Remote hosts become a transport change, not an architecture change.
