---
status: accepted
---
# A Surface sequences and styles each subscription separately, and a repeated Attach is a Resync

A Surface with more than one attached Client (an installed Client left running plus a second launch, or an `st` tool alongside the window) must give every Client a Delta that builds on the last frame *that Client* received. So `Delta.since_seq` is the subscription's own `last_sent_seq`, `Delta.new_styles` is the slice of the Style Table that subscription has not seen yet, a Snapshot for one Client never resets the table, and an `Attach` from a Client that is already attached is answered with a fresh Snapshot (a Resync) instead of an error. The wire encoding does not change; only what the fields mean.

## Considered options
- **One sequence per Surface, `since = seq − 1` for everyone** (what 1.1 does) — rejected: a subscription that had nothing pending during a flush falls one behind and reports a Gap on its next Delta; the Snapshot that recovers it resets the Style Table under every other subscription, which then paints unknown style indices as the default style ("all white"). Because the Gap's Resync was rejected as "already attached", the Client never Acked, the Server closed it after 30 s, and the reconnect Snapshot Gapped the other Client — a ping-pong observed in the daemon log on 2026-09-11.
- **Forbid multiple subscribers** — rejected: the Client is deliberately cheap to open twice (ADR 0002), tools Attach on the Data Plane, and a Client that is closing still holds its attachment for a moment.
- **Keep a private Style Table per subscription** — rejected: the table is append-only within a generation, so a per-subscription "sent up to index N" cursor gives the same result with one table and no copying. Overflow still resets the table and forces a Snapshot to every subscription, as before (Q45).
- **Per-subscription sequencing, one shared table with per-subscription cursors, Attach-as-Resync** — accepted.

## Consequences
- `Seq` is still one counter per Surface; only `since_seq` is per subscription. A Replica's Gap check is unchanged.
- `build_snapshot` no longer resets the table, so the table can hold up to its cap between overflows (about 48 KB per Surface at the cap). Acceptable.
- A Client may Resync at any time by re-sending `Attach`; the Server owes it a Snapshot within one flush. `st status` counts Resyncs so a Client that Resyncs repeatedly is visible.
- `docs/plan/02-protocol.md` §6 is updated to say so; protocol version stays 1.1.
