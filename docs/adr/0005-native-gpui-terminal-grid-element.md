---
status: accepted
---
# The cell grid is a native gpuix custom element, never React elements

gpuix moves React commits to Rust as JSON mutation batches (~592 B retained per element, ~30 ms to apply 220 k ops in its own benchmark). A 200×60 grid changing every frame would be tens of thousands of `setText`/`setStyle` mutations per frame — impossible in an 8 ms budget. We decided the grid is a Rust `CustomElement` (`<terminal-grid>`) that owns the Replica and paints glyph runs directly with GPUI, exactly like Zed's `terminal_view`. React only carries chrome and a `surfaceId` prop. Deltas flow Server → Rust and never touch JavaScript.

## Consequences
- We depend on gpuix's Rust‑only `CustomElementRegistry`, hence the vendoring decision in ADR‑0006.
- Keyboard routing: the element owns focus and encodes terminal keys in Rust; an allow‑list of app shortcuts is passed down so GPUI can dispatch them to React.
