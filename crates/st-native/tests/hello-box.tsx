// M0-08: renders <hello-box> from crates/st-native (NOT stock gpuix) and drives
// its props, proving the factory hook in patches/0001 puts our CustomElement in
// the registry GPUI actually renders from.
//
// Run it from the gpuix examples workspace, which is where @gpuix/react resolves
// until packages/app exists (M0-09):
//   cargo build -p st-native   # or just: cd crates/st-native && cargo build
//   cp crates/st-native/target/debug/libst_native.so \
//      crates/st-native/dist/superterminal-native.linux-x64-gnu.node
//   cp crates/st-native/tests/hello-box.tsx vendor/gpuix/examples/
//   cd vendor/gpuix/examples
//   NAPI_RS_NATIVE_LIBRARY_PATH=../../../crates/st-native/dist/superterminal-native.linux-x64-gnu.node \
//     bun hello-box.tsx
import React, { useEffect, useState } from "react"
import { createRenderer, render } from "@gpuix/react"

let setStateExternal: (s: { color: string; label: string }) => void = () => {}

function App() {
  const [s, setS] = useState({ color: "#3b82f6", label: "hello-box" })
  useEffect(() => {
    setStateExternal = setS
  }, [])
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        width: 400,
        height: 240,
        backgroundColor: "#11111b",
      }}
    >
      {/* @ts-expect-error custom element registered by st-native */}
      <hello-box color={s.color} label={s.label} />
    </div>
  )
}

const renderer = createRenderer()
renderer.init({ width: 400, height: 240, title: "st-hello-box" })
render(<App />, { renderer })

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

function helloBoxNode(): any {
  const tree = JSON.parse(renderer.getAutomationTree())
  let found: any = null
  const walk = (n: any) => {
    if (found) return
    if (n?.type === "hello-box") {
      found = n
      return
    }
    for (const c of n?.children ?? []) walk(c)
  }
  walk(tree)
  return found
}

async function main() {
  await sleep(1500)
  const mounted = helloBoxNode()
  console.log("HB mounted:", JSON.stringify(mounted))
  const w0 = mounted?.bounds?.width

  // Prop change #1: label. A different string re-shapes the text run, so the
  // element's measured width has to change — that is the observable proof the
  // prop reached Rust and forced a relayout.
  setStateExternal({ color: "#3b82f6", label: "a-much-longer-label" })
  await sleep(700)
  const relabelled = helloBoxNode()
  const w1 = relabelled?.bounds?.width
  console.log(`HB width ${w0} -> ${w1} after label change`)

  // Prop change #2: colour, 20 times. A quad carries no text, so the tree
  // cannot show the colour; what this proves is that 20 prop writes + repaints
  // do not take the UI thread down (after which every napi call would throw).
  for (let i = 0; i < 20; i++) {
    const hex = "#" + (0x100000 + i * 0x0a0a0a).toString(16).slice(-6)
    setStateExternal({ color: hex, label: "a-much-longer-label" })
    await sleep(40)
  }
  await sleep(400)
  const alive = helloBoxNode()
  console.log("HB alive after 20 colour changes:", alive ? "YES" : "NO")

  const pass = !!mounted?.bounds && w1 > w0 && !!alive?.bounds
  console.log(pass ? "HELLO-BOX RESULT: PASS" : "HELLO-BOX RESULT: FAIL")
  process.exit(pass ? 0 : 1)
}

main()
