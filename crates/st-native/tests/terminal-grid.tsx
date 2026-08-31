// M2 / HANDOVER V4: the `<terminal-grid>` CustomElement end to end — it mounts,
// it sizes itself from the font, a prop change re-derives the grid, a Data Plane
// Snapshot lands, and unmounting takes the Surface back out of the registry.
//
// Linux has no pixel introspection (`docs/DEV.md` §4): `getPaintedText()` and
// `getAllText()` return `[]`, `captureScreenshot()` refuses, and the automation
// tree carries only type/id/bounds for a custom element. So nothing here looks
// at painted glyphs. It asserts on two things that *are* observable: the painted
// bounds in the automation tree, and the state the element publishes for
// `stReadProp` (`crates/st-native/src/registry.rs`).
//
// The geometry assertion is derived, never hard-coded: `GridGeometry::fit`
// (`crates/st-native/src/geometry.rs`) is re-implemented below in f32 arithmetic
// and fed the *measured* bounds and the *read-back* cell size, so the test says
// the same thing on a box whose "monospace" resolves to a different face.
//
// Run it from the gpuix examples workspace, which is where @gpuix/react resolves
// until packages/app exists (M0-09):
//   cd crates/st-native && cargo build
//   cp target/debug/libst_native.so dist/superterminal-native.linux-x64-gnu.node
//   cargo build --example fake_dataplane        # so the test does not shell out to cargo
//   cd ../.. && cp crates/st-native/tests/terminal-grid.tsx vendor/gpuix/examples/
//   cd vendor/gpuix/examples
//   NAPI_RS_NATIVE_LIBRARY_PATH=../../../crates/st-native/dist/superterminal-native.linux-x64-gnu.node \
//     bun terminal-grid.tsx
//
// The Data Plane half is served by `crates/st-native/examples/fake_dataplane.rs`,
// spawned here and killed at the end. Point `ST_FAKE_DATAPLANE` at a prebuilt
// binary to skip the `cargo run` fallback (which will happily spend three
// minutes compiling GPUI the first time).
import React, { useEffect, useState } from "react"
import { createRenderer, render } from "@gpuix/react"
import { createRequire } from "node:module"
import { existsSync } from "node:fs"
import { dirname, join } from "node:path"
import { tmpdir } from "node:os"

// Our own napi reads live on the same addon `@gpuix/react` already dlopen'ed
// through NAPI_RS_NATIVE_LIBRARY_PATH — gpuix's index.js does
// `module.exports = nativeBinding`, so every `#[napi]` in st-native is on it.
// `createRequire` rather than a bare import: the addon is CJS and we want the
// module.exports object itself, not Bun's ESM interop guess at it.
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
  stReadableProps: () => string[]
  stDataPlanePaths: () => string[]
}

const bun = (globalThis as any).Bun

// ── the fixture ──────────────────────────────────────────────────────
// A parent div with an exact pixel size, so the element's painted bounds are a
// number this test can also reason about, and a padding that is *not* the
// `Padding::default()` of {4,4,4,6} — a default that happened to match would
// hide a padding that never reached Rust.
const SURFACE_ID = 1
const BOX_W = 800
const BOX_H = 400
const PADDING = { top: 6, right: 8, bottom: 6, left: 10 }
const LINE_HEIGHT = 1.2
const FONT_SMALL = 14
const FONT_LARGE = 22
// The fake server's grid. BOX_H/FONT_SMALL are chosen so the rows we fit
// (floor((400-12)/16.8) = 23) stay under it, which keeps `contentLines >= rows`
// true whether or not the stub honours our Resize.
const SERVER_COLS = 80
const SERVER_ROWS = 24

let setFontSizeExternal: (px: number) => void = () => {}
let setMountedExternal: (mounted: boolean) => void = () => {}

function App({ socketPath }: { socketPath: string }) {
  const [fontSize, setFontSize] = useState(FONT_SMALL)
  const [mounted, setMounted] = useState(true)
  useEffect(() => {
    setFontSizeExternal = setFontSize
    setMountedExternal = setMounted
  }, [])
  return (
    <div
      style={{
        display: "flex",
        width: BOX_W,
        height: BOX_H,
        backgroundColor: "#11111b",
      }}
    >
      {mounted && (
        // @ts-expect-error custom element registered by st-native
        <terminal-grid
          surfaceId={SURFACE_ID}
          socketPath={socketPath}
          attachMode="active"
          fontFamily="monospace"
          fontSize={fontSize}
          lineHeight={LINE_HEIGHT}
          padding={PADDING}
          cursorStyle="block"
          cursorBlink={false}
          scrollbar="never"
          style={{ width: BOX_W, height: BOX_H }}
        />
      )}
    </div>
  )
}

// ── GridGeometry::fit, in f32 ────────────────────────────────────────
// Rust does this arithmetic in f32 and JS numbers are f64, so every step is
// rounded through Math.fround. Without it a division that lands a hair under a
// cell boundary floors differently in the two languages and this test fails on
// one font and passes on the next.
const f32 = Math.fround
const MAX_CELLS = 1000

function fitCells(extent: number, padA: number, padB: number, cell: number): number {
  const usable = Math.max(f32(f32(f32(extent) - padA) - padB), 0)
  const whole = Math.floor(f32(usable / f32(cell)))
  return Math.min(Math.max(whole, 1), MAX_CELLS)
}

// ── harness ──────────────────────────────────────────────────────────
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))
const failures: string[] = []

function check(name: string, ok: boolean, detail: string) {
  console.log(`${ok ? "  ok  " : " FAIL "} ${name} — ${detail}`)
  if (!ok) failures.push(name)
}

function gridNode(renderer: any): any {
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

// ── the fake Data Plane ──────────────────────────────────────────────
function repoRoot(): string {
  // The script runs from either crates/st-native/tests or, once copied,
  // vendor/gpuix/examples. Both are inside the repo, so walk up to the marker.
  let dir = import.meta.dir
  for (let i = 0; i < 10; i++) {
    if (existsSync(join(dir, "crates", "st-native", "Cargo.toml"))) return dir
    dir = dirname(dir)
  }
  throw new Error(`cannot find the repo root above ${import.meta.dir}`)
}

function fakeServerCommand(socketPath: string): { cmd: string[]; cwd: string } {
  const crate = join(repoRoot(), "crates", "st-native")
  const args = [
    socketPath,
    "--cols",
    String(SERVER_COLS),
    "--rows",
    String(SERVER_ROWS),
    "--surface",
    String(SURFACE_ID),
    "--text",
    "hello world|second line",
  ]
  const override = process.env.ST_FAKE_DATAPLANE
  if (override) return { cmd: [override, ...args], cwd: crate }
  const prebuilt = join(crate, "target", "debug", "examples", "fake_dataplane")
  if (existsSync(prebuilt)) return { cmd: [prebuilt, ...args], cwd: crate }
  return {
    cmd: ["cargo", "run", "--quiet", "--example", "fake_dataplane", "--", ...args],
    cwd: crate,
  }
}

const TIMED_OUT = Symbol("timeout")

/** Waits for the stub's `READY <path>` line. `null` on timeout or early exit. */
async function waitForReady(proc: any, timeoutMs: number): Promise<string | null> {
  const reader = proc.stdout.getReader()
  const decoder = new TextDecoder()
  const deadline = Date.now() + timeoutMs
  let buffer = ""
  try {
    for (;;) {
      const remaining = deadline - Date.now()
      if (remaining <= 0) return null
      let timer: any
      const timeout = new Promise((r) => {
        timer = setTimeout(() => r(TIMED_OUT), remaining)
      })
      const result: any = await Promise.race([reader.read(), timeout])
      clearTimeout(timer)
      if (result === TIMED_OUT || result.done) return null
      buffer += decoder.decode(result.value, { stream: true })
      const line = buffer.split("\n").find((l) => l.startsWith("READY"))
      if (line) return line.trim()
    }
  } finally {
    reader.releaseLock()
    // Keep draining, or a chatty stub eventually blocks on a full pipe.
    proc.stdout.pipeTo(new WritableStream({ write() {} })).catch(() => {})
  }
}

async function main() {
  const socketPath = join(tmpdir(), `st-terminal-grid-${process.pid}.sock`)
  const { cmd, cwd } = fakeServerCommand(socketPath)
  console.log("TG fake dataplane:", cmd.join(" "))
  const server = bun.spawn(cmd, { cwd, stdout: "pipe", stderr: "inherit" })

  try {
    const ready = await waitForReady(server, 240_000)
    check("fake dataplane is listening", ready !== null, ready ?? "no READY line")
    if (ready === null) {
      // process.exit skips `finally`, so the stub has to be reaped here.
      server.kill()
      console.log("TERMINAL-GRID RESULT: FAIL")
      process.exit(1)
    }

    const renderer = createRenderer()
    renderer.init({ width: 900, height: 520, title: "st-terminal-grid" })
    render(<App socketPath={socketPath} />, { renderer })
    await sleep(2500)

    console.log("TG readable props:", JSON.stringify(native.stReadableProps()))
    console.log("TG data plane paths:", JSON.stringify(native.stDataPlanePaths()))

    // ── 1. it is in the tree, with real bounds ──────────────────────
    const node = gridNode(renderer)
    const bounds = node?.bounds
    check(
      "terminal-grid is in the automation tree",
      !!node,
      node ? `id=${node.id}` : "no node with type terminal-grid"
    )
    check(
      "it has non-zero painted bounds matching the parent box",
      !!bounds && bounds.width === BOX_W && bounds.height === BOX_H,
      JSON.stringify(bounds ?? null)
    )
    if (!bounds) {
      server.kill()
      console.log("TERMINAL-GRID RESULT: FAIL")
      process.exit(1)
    }

    // ── 2. size == GridGeometry::fit(bounds, cellSize, padding) ─────
    // The real geometry proof. `cellSize` comes from the font gpui actually
    // resolved, so this holds whatever "monospace" turns out to be here.
    const cell0 = native.stReadProp(SURFACE_ID, "cellSize")
    const size0 = native.stReadProp(SURFACE_ID, "size")
    if (!cell0 || !size0) {
      // A null here means the element never published for this Surface at all,
      // which makes every assertion below meaningless rather than failing.
      server.kill()
      console.log(`TG no published snapshot for surface ${SURFACE_ID}`)
      console.log("TERMINAL-GRID RESULT: FAIL")
      process.exit(1)
    }
    check(
      "cellSize reads back non-degenerate",
      cell0.w > 1 && cell0.h > 1,
      JSON.stringify(cell0)
    )
    // `resolve_cell` sets the cell height to font_size * line_height exactly,
    // so a wrong lineHeight is visible here rather than only in the row count.
    check(
      "cell height is fontSize * lineHeight",
      Math.abs(cell0.h - f32(FONT_SMALL * LINE_HEIGHT)) < 0.01,
      `${cell0.h} vs ${f32(FONT_SMALL * LINE_HEIGHT)}`
    )
    const wantCols0 = fitCells(bounds.width, PADDING.left, PADDING.right, cell0.w)
    const wantRows0 = fitCells(bounds.height, PADDING.top, PADDING.bottom, cell0.h)
    check(
      "size == GridGeometry::fit(bounds, cellSize, padding)",
      size0.cols === wantCols0 && size0.rows === wantRows0,
      `got ${size0.cols}x${size0.rows}, derived ${wantCols0}x${wantRows0} ` +
        `from ${bounds.width}x${bounds.height} px, cell ${cell0.w}x${cell0.h}, ` +
        `padding ${JSON.stringify(PADDING)}`
    )

    // ── 3. the Data Plane actually connected and a Snapshot landed ──
    const connected = native.stReadProp(SURFACE_ID, "connected")
    const attached = native.stReadProp(SURFACE_ID, "attached")
    const contentLines = native.stReadProp(SURFACE_ID, "contentLines")
    check("connected", connected === true, String(connected))
    check("attached", attached === true, String(attached))
    // content_lines is history + the visible grid, and stays 0 until a Snapshot
    // is applied to the Replica — so "at least a screenful" *is* the proof one
    // arrived, not merely that the socket opened.
    check(
      "contentLines >= rows (a Snapshot arrived)",
      typeof contentLines === "number" && contentLines >= size0.rows,
      `contentLines=${contentLines}, rows=${size0.rows}`
    )
    console.log("TG title:", JSON.stringify(native.stReadProp(SURFACE_ID, "title")))
    console.log("TG modes:", JSON.stringify(native.stReadProp(SURFACE_ID, "modes")))

    // ── 4. frames were painted ──────────────────────────────────────
    const stats = native.stReadProp(SURFACE_ID, "stats")
    check("stats.frames > 0", (stats?.frames ?? 0) > 0, JSON.stringify(stats))
    console.log(
      `TG TIMINGS lastFrameMs=${stats?.lastFrameMs} p95FrameMs=${stats?.p95FrameMs} ` +
        `runCacheHitRate=${stats?.runCacheHitRate} ` +
        `(shaped=${stats?.shapedRuns} cached=${stats?.cachedRuns} len=${stats?.runCacheLen})`
    )
    console.log(
      "TG TIMINGS are GL-on-WSLg, NOT gate-worthy (grilling Q52): WSLg composites " +
        "through RDP on a D3D12-backed GL fallback, so these numbers cannot decide " +
        "the M2 rendering gate. Reported for drift-watching only."
    )

    // ── 5. a prop change re-derives the geometry ────────────────────
    setFontSizeExternal(FONT_LARGE)
    await sleep(1200)
    const cell1 = native.stReadProp(SURFACE_ID, "cellSize")
    const size1 = native.stReadProp(SURFACE_ID, "size")
    check(
      "a bigger fontSize grows the cell",
      cell1.w > cell0.w && cell1.h > cell0.h,
      `${cell0.w}x${cell0.h} -> ${cell1.w}x${cell1.h}`
    )
    check(
      "a bigger cell shrinks the grid",
      size1.cols < size0.cols && size1.rows < size0.rows,
      `${size0.cols}x${size0.rows} -> ${size1.cols}x${size1.rows}`
    )
    // …and it shrank to exactly what fit() says, not merely in the right
    // direction: this is what catches a stale cell cache or a padding that is
    // applied once at mount and never again.
    const wantCols1 = fitCells(bounds.width, PADDING.left, PADDING.right, cell1.w)
    const wantRows1 = fitCells(bounds.height, PADDING.top, PADDING.bottom, cell1.h)
    check(
      "the new size == GridGeometry::fit with the new cell",
      size1.cols === wantCols1 && size1.rows === wantRows1,
      `got ${size1.cols}x${size1.rows}, derived ${wantCols1}x${wantRows1}`
    )

    // ── 6. the Surface registry tracks the element's lifetime ───────
    const listedWhileMounted = native.stListGrids()
    check(
      "stListGrids contains the surface while mounted",
      listedWhileMounted.includes(SURFACE_ID),
      JSON.stringify(listedWhileMounted)
    )
    setMountedExternal(false)
    await sleep(1200)
    const listedAfterUnmount = native.stListGrids()
    // `destroy()` calls registry::retire; a Surface still listed here means a
    // leaked snapshot, and with it a leaked DataPlaneHandle.
    check(
      "stListGrids drops the surface after unmount",
      !listedAfterUnmount.includes(SURFACE_ID),
      JSON.stringify(listedAfterUnmount)
    )
    // Loose ==: napi maps `Option::None` to null, but a version bump flipping it
    // to undefined would be a spurious failure, not a regression in the element.
    const afterUnmount = native.stReadProp(SURFACE_ID, "size")
    check(
      "stReadProp returns nothing for the unmounted surface",
      afterUnmount == null,
      JSON.stringify(afterUnmount ?? null)
    )
  } finally {
    server.kill()
  }

  const pass = failures.length === 0
  console.log(pass ? "TERMINAL-GRID RESULT: PASS" : `TERMINAL-GRID RESULT: FAIL (${failures.join(", ")})`)
  process.exit(pass ? 0 : 1)
}

main()
