// M0-04 / HANDOVER V1: the gpuix counter under Bun, driven through GPUI's own
// input pipeline. This is the ThreadsafeFunction re-entry test: every
// simulateClick dispatches on the GPUI thread, which calls back into Bun's JS
// thread to run React's setState, which calls back into native applyBatch.
//
// Run it from the gpuix examples workspace, which is where @gpuix/react resolves
// until packages/app exists (M0-09):
//   cp crates/st-native/tests/v1-counter.tsx vendor/gpuix/examples/
//   cd vendor/gpuix/examples && bun v1-counter.tsx
//
// Requires patches/0002 — without it the first simulateClick panics the UI thread.
import React, { useState } from "react"
import { createRenderer, render } from "@gpuix/react"

function Counter() {
  const [count, setCount] = useState(0)
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: 16,
        padding: 32,
        width: 400,
        height: 300,
        backgroundColor: "#1e1e2e",
      }}
    >
      <div
        style={{ fontSize: 48, color: "#cdd6f4", cursor: "pointer" }}
        onClick={() => setCount((c) => c + 1)}
      >
        {`count=${count}`}
      </div>
    </div>
  )
}

const renderer = createRenderer()
renderer.init({ width: 400, height: 300, title: "v1-counter" })
render(<Counter />, { renderer })

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

function findCountNode(): any {
  const tree = JSON.parse(renderer.getAutomationTree())
  let found: any = null
  const walk = (n: any) => {
    if (found) return
    const text = Array.isArray(n?.text) ? n.text.join("") : (n?.text ?? "")
    if (typeof text === "string" && text.startsWith("count=")) { found = n; return }
    for (const c of n?.children ?? []) walk(c)
  }
  walk(tree)
  return found
}

function nodeText(n: any): string {
  return Array.isArray(n?.text) ? n.text.join("") : (n?.text ?? "")
}

function findClickTarget(): [number, number] | null {
  const tree = JSON.parse(renderer.getAutomationTree())
  let found: [number, number] | null = null
  const walk = (n: any) => {
    if (found) return
    const text = Array.isArray(n?.text) ? n.text.join("") : (n?.text ?? "")
    if (typeof text === "string" && text.startsWith("count=") && n.bounds) {
      const b = n.bounds
      const x = (b.x ?? b[0]) + (b.width ?? b[2]) / 2
      const y = (b.y ?? b[1]) + (b.height ?? b[3]) / 2
      found = [x, y]
      return
    }
    for (const c of n?.children ?? []) walk(c)
  }
  walk(tree)
  return found
}

async function main() {
  await sleep(1500)
  console.log("V1 automation-tree text after mount:", JSON.stringify(nodeText(findCountNode())))

  const target = findClickTarget()
  console.log("V1 click target:", JSON.stringify(target))
  const [x, y] = target ?? [200, 150]

  const t0 = performance.now()
  for (let i = 0; i < 50; i++) {
    renderer.simulateClick(x, y)
    // Let the reconciler flush the state update back into native.
    await sleep(20)
  }
  const elapsed = performance.now() - t0
  await sleep(500)

  console.log(`V1 50 clicks in ${elapsed.toFixed(0)} ms`)
  const after = nodeText(findCountNode())
  console.log("V1 automation-tree text after 50 clicks:", JSON.stringify(after))
  console.log(after === "count=50" ? "V1 RESULT: PASS" : `V1 RESULT: FAIL (${after})`)

  process.exit(0)
}

main()
