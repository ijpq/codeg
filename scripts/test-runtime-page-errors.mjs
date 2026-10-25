import assert from "node:assert/strict"
import { test } from "node:test"
import { EventEmitter, once } from "node:events"
import { createHash } from "node:crypto"
import { readFileSync } from "node:fs"
import {
  correlateRuntimeErrors,
  isEmbeddedTauriPathError,
  scopedRuntimeErrors,
} from "./runtime-page-errors.mjs"

const known = {
  mainFrame: false,
  message: "TypeError: Cannot read properties of undefined (reading 'plugins')",
  source: readFileSync(
    new URL("./fixtures/tauri-2.12.4-path-windows.txt", import.meta.url),
    "utf8"
  ),
}

test("the pinned framework fixture matches the actual Windows child script", () => {
  assert.equal(
    createHash("sha256").update(known.source).digest("hex"),
    "aa46bd912cdbc5668bd706619e8f7de659d00d5af8512889de3f1ba61896cc71"
  )
  assert.match(known.source, /JSON\.parse/)
})

test("only the exact Tauri path initializer in a confirmed child frame is classified", () => {
  assert.equal(isEmbeddedTauriPathError(known), true)
  for (const change of [
    { mainFrame: true },
    { mainFrame: null },
    { source: undefined },
    { source: "app.plugins.start()" },
    { source: known.source.replace("JSON.parse", "JSON.other") },
    { source: `${known.source} throw new Error('app');` },
    { message: "TypeError: actual document conversion failure" },
  ])
    assert.equal(isEmbeddedTauriPathError({ ...known, ...change }), false)
})

test("application, unknown and uncorrelated errors remain failures", () => {
  const result = correlateRuntimeErrors(
    [known.message, known.message, "Unobserved renderer error"],
    [known, { ...known, mainFrame: true }]
  )
  assert.equal(result.embeddedFrameworkErrors.length, 1)
  assert.equal(result.errors.length, 2)
  assert.equal(result.rawErrors.length, 3)
  assert.equal(result.errors[0].mainFrame, true)
  assert.equal(result.errors[1].uncorrelated, true)
  assert.equal("source" in result.embeddedFrameworkErrors[0], false)
})

test("CDP frame identity and source are required and the scope closes once", async () => {
  const client = new EventEmitter()
  let detached = 0
  client.send = async (method) => {
    if (method === "Page.getFrameTree")
      return { frameTree: { frame: { id: "app" } } }
    if (method === "Debugger.getScriptSource")
      return { scriptSource: known.source }
    return {}
  }
  client.detach = async () => detached++
  const page = new EventEmitter()
  page.context = () => ({ newCDPSession: async () => client })
  const scope = await scopedRuntimeErrors(page)
  client.emit("Runtime.executionContextCreated", {
    context: {
      id: 9,
      origin: "http://127.0.0.1:49152/?cap=secret",
      auxData: { frameId: "office" },
    },
  })
  page.emit("pageerror", new TypeError(known.message.slice(11)))
  client.emit("Runtime.exceptionThrown", {
    exceptionDetails: {
      executionContextId: 9,
      exception: { description: `${known.message}\n    at <anonymous>:5:50` },
      stackTrace: { callFrames: [{ scriptId: "path-init" }] },
    },
  })
  const result = await scope.stop()
  assert.deepEqual(result.errors, [])
  assert.equal(result.embeddedFrameworkErrors.length, 1)
  assert.equal(
    result.embeddedFrameworkErrors[0].origin,
    "http://127.0.0.1:49152"
  )
  page.emit("pageerror", new Error("later isolated scroll fixture"))
  assert.strictEqual(await scope.stop(), result)
  assert.equal(detached, 1)
  assert.equal(page.listenerCount("pageerror"), 0)
})

test("out-of-process iframe exceptions are inspected before the child resumes", async () => {
  const client = new EventEmitter()
  const page = new EventEmitter()
  const commands = []
  const completed = once(client, "child-exception-observed")
  const emitChild = (method, params) =>
    client.emit("Target.receivedMessageFromTarget", {
      sessionId: "office-session",
      message: JSON.stringify({ method, params }),
    })
  client.send = async (method, params) => {
    if (method === "Page.getFrameTree")
      return { frameTree: { frame: { id: "app" } } }
    if (method === "Target.setAutoAttach" && params.autoAttach)
      queueMicrotask(() =>
        client.emit("Target.attachedToTarget", {
          sessionId: "office-session",
          targetInfo: { type: "iframe", targetId: "office" },
        })
      )
    if (method === "Target.sendMessageToTarget") {
      const packet = JSON.parse(params.message)
      commands.push(packet.method)
      queueMicrotask(() => {
        if (packet.method === "Runtime.enable")
          emitChild("Runtime.executionContextCreated", {
            context: { id: 9, auxData: { frameId: "office" } },
          })
        client.emit("Target.receivedMessageFromTarget", {
          sessionId: "office-session",
          message: JSON.stringify({
            id: packet.id,
            result:
              packet.method === "Debugger.getScriptSource"
                ? { scriptSource: known.source }
                : {},
          }),
        })
        if (packet.method === "Runtime.runIfWaitingForDebugger") {
          page.emit("pageerror", new TypeError(known.message.slice(11)))
          emitChild("Runtime.exceptionThrown", {
            exceptionDetails: {
              executionContextId: 9,
              exception: { description: known.message },
              stackTrace: { callFrames: [{ scriptId: "path-init" }] },
            },
          })
          client.emit("child-exception-observed")
        }
      })
    }
    return {}
  }
  client.detach = async () => {}
  page.context = () => ({ newCDPSession: async () => client })
  const scope = await scopedRuntimeErrors(page)
  await completed
  const result = await scope.stop()
  assert.deepEqual(result.errors, [])
  assert.equal(result.embeddedFrameworkErrors.length, 1)
  assert.equal(result.embeddedFrameworkErrors[0].frameId, "office")
  assert.ok(
    commands.indexOf("Debugger.enable") <
      commands.indexOf("Runtime.runIfWaitingForDebugger")
  )
  assert.ok(commands.includes("Debugger.getScriptSource"))
  assert.equal(client.listenerCount("Target.receivedMessageFromTarget"), 0)
})
