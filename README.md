# superterminal

A GPU‑rendered, native multiplexer terminal for Windows, Linux and Mac. Rust server (`superterminald`) owns the terminals; a Bun 1.4.0 + React client renders them through [gpuix](https://github.com/remorses/gpuix) (React bindings for Zed's GPUI) with a native Rust `<terminal-grid>` element.

**Status: planning only — no implementation yet.** Start with [`HANDOVER.md`](./HANDOVER.md).

## Documents

| File | Purpose |
|---|---|
| [`HANDOVER.md`](./HANDOVER.md) | Entry point for an AI agent (or human) picking up implementation |
| [`CONTEXT.md`](./CONTEXT.md) | Ubiquitous language / glossary — use these words everywhere |
| [`docs/plan/00-grilling.md`](./docs/plan/00-grilling.md) | The 36 decisions, with reasoning, that everything else depends on |
| [`docs/plan/01-architecture.md`](./docs/plan/01-architecture.md) | Processes, threads, connections, crate layout, failure modes |
| [`docs/plan/02-protocol.md`](./docs/plan/02-protocol.md) | Wire protocol: Control Plane (JSON) and Data Plane (binary) |
| [`docs/plan/03-server.md`](./docs/plan/03-server.md) | `superterminald`: workspace actor, VT engine, PTYs, persistence |
| [`docs/plan/04-client-native.md`](./docs/plan/04-client-native.md) | Rust native module: gpuix patch, Replica, `<terminal-grid>` painting & input |
| [`docs/plan/05-client-app.md`](./docs/plan/05-client-app.md) | Bun/React chrome: tabs, sessions, palette, control‑plane client, packaging |
| [`docs/plan/06-testing-perf-ci.md`](./docs/plan/06-testing-perf-ci.md) | Test pyramid, VT conformance, perf budgets, CI |
| [`docs/plan/07-milestones.md`](./docs/plan/07-milestones.md) | M0–M6 work breakdown with task ids, estimates, acceptance tests |
| [`docs/adr/`](./docs/adr/) | Architecture decision records (the hard‑to‑reverse choices) |
