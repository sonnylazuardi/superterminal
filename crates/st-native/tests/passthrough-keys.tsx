// M2 / HANDOVER V5: does a key the CustomElement *declines* reach a React
// ancestor's `onKeyDown`?
//
// 04 §7 (grilling Q23) is built on one assumption: `passthroughKeys` chords get
// `KeyOutcome::Passthrough`, the element deliberately does NOT call
// `cx.stop_propagation()`, and GPUI therefore keeps bubbling the key up the
// focus chain until it hits the ancestor div React put a `keyDown` listener on.
// Every other key is consumed and never reaches React at all. If that bubbling
// does not happen, the whole command layer (cmd-T, cmd-shift-]) has to be
// rebuilt on the `shortcut` event instead — so this script measures BOTH
// mechanisms and reports them separately rather than collapsing them into one
// verdict. Whoever runs it records which one is actually in use.
//
//   V5 BUBBLING       — ctrl-shift-t reached the ancestor div's onKeyDown
//   V5 SHORTCUT EVENT — ctrl-shift-t arrived as the element's `shortcut` event
//
// Requires `patches/0002` — without it the first `simulateClick` panics the
// gpuix-ui thread with "cannot update GpuixView while it is already being
// updated", and every napi call after that throws.
//
// No Data Plane server is needed: `handle_key` classifies and `send()` is a
// no-op with no connection, but `cx.stop_propagation()` still runs, which is
// the thing under test.
//
// Run it from the gpuix examples workspace, which is where @gpuix/react resolves
// until packages/app exists (M0-09):
//   cd crates/st-native && cargo build
//   cp target/debug/libst_native.so dist/superterminal-native.linux-x64-gnu.node
//   cd ../.. && cp crates/st-native/tests/passthrough-keys.tsx vendor/gpuix/examples/
//   cd vendor/gpuix/examples
//   NAPI_RS_NATIVE_LIBRARY_PATH=../../../crates/st-native/dist/superterminal-native.linux-x64-gnu.node \
//     bun passthrough-keys.tsx
import React, { useEffect, useState } from "react"
import { createRenderer, render } from "@gpuix/react"
import { createRequire } from "node:module"

const requireCjs = createRequire(import.meta.url)
// `@gpuix/native` is not resolvable from the examples workspace, and pointing
// at it would in any case load the *stock* addon rather than ours. Require the
// exact file `@gpuix/react` was told to load, which is the same handle the
// GPUI runtime lives behind (docs/DEV.md §5).
const addonPath = process.env.NAPI_RS_NATIVE_LIBRARY_PATH
if (!addonPath) {
  console.error("NAPI_RS_NATIVE_LIBRARY_PATH must point at superterminal-native.<triple>.node")
  process.exit(1)
}
const native = requireCjs(addonPath) as {
  stReadProp: (surfaceId: number, key: string) => any
  stListGrids: () => number[]
}

const SURFACE_ID = 1
const BOX_W = 700
const BOX_H = 400
const PASSTHROUGH = "ctrl-shift-t" // claimed by React
const CONSUMED = "ctrl-c" // a real terminal key: must never reach React

// Everything the ancestor div saw, as chord strings in the same spelling
// `crate::input::chord_string` uses, so the two channels are comparable.
const ancestorKeys: string[] = []
// Everything the element emitted as a `shortcut` event.
const shortcutEvents: string[] = []
const focusEvents: string[] = []

function chordOf(event: any): string {
  const m = event?.modifiers ?? {}
  let out = ""
  if (m.ctrl) out += "ctrl-"
  if (m.alt) out += "alt-"
  if (m.shift) out += "shift-"
  if (m.cmd) out += "cmd-"
  return out + (event?.key ?? "?")
}

let bumpExternal: () => void = () => {}

function App() {
  // Any prop write forces the retained tree to mark the element changed, which
  // is how `setEventListener` below gets picked up — see the comment at its
  // call site.
  const [bump, setBump] = useState(0)
  useEffect(() => {
    bumpExternal = () => setBump((n) => n + 1)
  }, [])
  return (
    <div
      style={{
        display: "flex",
        width: BOX_W,
        height: BOX_H,
        backgroundColor: "#11111b",
      }}
      // A div with onKeyDown is focusable in gpuix (`sync_focus_handles`), so it
      // sits in the focus chain above the grid and is exactly the React ancestor
      // Q23 is about.
      onKeyDown={(event: any) => {
        ancestorKeys.push(chordOf(event))
      }}
    >
      {/* @ts-expect-error custom element registered by st-native */}
      <terminal-grid
        surfaceId={SURFACE_ID}
        fontFamily="monospace"
        fontSize={14}
        lineHeight={1.2}
        cursorBlink={false}
        scrollbar="never"
        passthroughKeys={[PASSTHROUGH]}
        // `onShortcut` is NOT in @gpuix/react's EVENT_PROPS table, so the
        // reconciler forwards it as a custom prop (a function, serialised to
        // null) and never calls setEventListener for it. Declared anyway to
        // document the intended API; the listener is registered by hand below.
        onShortcut={(event: any) => {
          shortcutEvents.push(String(event?.value ?? chordOf(event)))
        }}
        // These two ARE in EVENT_PROPS, and declaring them is what makes gpuix
        // create a focus handle for this element id — without one,
        // `renderer.focusElement(id)` has nothing to focus.
        onFocus={() => focusEvents.push("focus")}
        onBlur={() => focusEvents.push("blur")}
        data-bump={bump}
        style={{ width: BOX_W, height: BOX_H }}
      />
    </div>
  )
}

// The raw event stream is the reliable way to see `shortcut`: gpuix's React
// reconciler only routes the event types in its own EVENT_PROPS list, and
// `shortcut` is ours, not gpuix's.
const renderer = createRenderer((event: any) => {
  if (event?.eventType === "shortcut") {
    shortcutEvents.push(String(event.value ?? chordOf(event)))
  }
})
renderer.init({ width: BOX_W, height: BOX_H, title: "st-passthrough-keys" })
render(<App />, { renderer })

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))
const failures: string[] = []

function check(name: string, ok: boolean, detail: string) {
  console.log(`${ok ? "  ok  " : " FAIL "} ${name} — ${detail}`)
  if (!ok) failures.push(name)
}

function gridNode(): any {
  const tree = JSON.parse(renderer.getAutomationTree())
  let found: any = null
  const walk = (n: any) => {
    if (found || !n) return
    if (n.type === "terminal-grid") {
      found = n
      return
    }
    for (const c of n.children ?? []) walk(c)
  }
  walk(tree)
  return found
}

async function main() {
  await sleep(2000)

  const node = gridNode()
  check(
    "terminal-grid is in the automation tree",
    !!node?.bounds,
    JSON.stringify(node ?? null)
  )
  if (!node?.bounds) {
    console.log("PASSTHROUGH RESULT: FAIL")
    process.exit(1)
  }
  // Not asserted, only reported: it confirms the element really is live in the
  // registry, so a silent "no keys anywhere" result cannot be a dead element.
  console.log(
    `V5 grid surfaces=${JSON.stringify(native.stListGrids())} ` +
      `size=${JSON.stringify(native.stReadProp(SURFACE_ID, "size"))}`
  )

  // Declare the `shortcut` listener straight on the retained tree. React cannot:
  // its EVENT_PROPS table is a fixed list of gpuix's own event names and has no
  // entry for ours, so `<terminal-grid onShortcut>` never produces a
  // setEventListener op. The element checks `declared_events` before emitting,
  // so without this line `emit("shortcut", …)` returns early and the fallback
  // mechanism would look broken when it is only unsubscribed.
  renderer.applyBatch(JSON.stringify([["setEventListener", node.id, "shortcut", true]]))
  // set_event_listener does not itself mark the element dirty, and the element
  // only re-reads ctx.events inside render() — so push a prop change through
  // React to force one.
  bumpExternal()
  await sleep(600)

  // Focus the grid. Both routes, because they exercise different code:
  // simulateClick goes through the element's own on_mouse_down (which calls
  // focus.focus()), focusElement goes through the handle gpuix created for the
  // onFocus/onBlur listeners.
  const cx = node.bounds.x + node.bounds.width / 2
  const cy = node.bounds.y + node.bounds.height / 2
  renderer.simulateClick(cx, cy)
  await sleep(300)
  renderer.focusElement(node.id)
  await sleep(500)
  console.log(`V5 focus events on the grid: ${JSON.stringify(focusEvents)}`)
  check(
    "the grid took focus",
    focusEvents.includes("focus"),
    `click at ${cx},${cy} then focusElement(${node.id})`
  )
  check(
    "nothing reached the ancestor before any key was pressed",
    ancestorKeys.length === 0,
    JSON.stringify(ancestorKeys)
  )

  // ── the declined chord ────────────────────────────────────────────
  renderer.simulateKeyDown(PASSTHROUGH)
  await sleep(600)
  const bubbled = ancestorKeys.includes(PASSTHROUGH)
  const shortcut = shortcutEvents.includes(PASSTHROUGH)
  console.log(`V5 ancestor onKeyDown saw: ${JSON.stringify(ancestorKeys)}`)
  console.log(`V5 shortcut events saw:    ${JSON.stringify(shortcutEvents)}`)
  console.log(`V5 BUBBLING: ${bubbled ? "YES" : "NO"}`)
  console.log(`V5 SHORTCUT EVENT: ${shortcut ? "YES" : "NO"}`)
  // Either mechanism is a working command layer; neither is a redesign.
  check(
    `${PASSTHROUGH} reached React by at least one mechanism`,
    bubbled || shortcut,
    `bubbling=${bubbled} shortcutEvent=${shortcut}`
  )

  // ── the consumed chord ────────────────────────────────────────────
  // ctrl-c encodes to 0x03 and is the single most important key a terminal must
  // not leak: if this one bubbles, every ctrl chord is also a React shortcut and
  // the passthrough list means nothing.
  const beforeConsumed = ancestorKeys.length
  renderer.simulateKeyDown(CONSUMED)
  await sleep(600)
  const leaked = ancestorKeys.slice(beforeConsumed)
  console.log(`V5 CONSUMED ${CONSUMED}: ${leaked.length === 0 ? "YES" : "NO"}`)
  check(
    `${CONSUMED} was consumed by the element`,
    leaked.length === 0,
    `ancestor saw ${JSON.stringify(leaked)}`
  )
  check(
    `${CONSUMED} did not fire a shortcut event`,
    !shortcutEvents.includes(CONSUMED),
    JSON.stringify(shortcutEvents)
  )

  const pass = failures.length === 0
  console.log(pass ? "PASSTHROUGH RESULT: PASS" : `PASSTHROUGH RESULT: FAIL (${failures.join(", ")})`)
  process.exit(pass ? 0 : 1)
}

main()
