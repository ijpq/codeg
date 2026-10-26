import { createHash } from "node:crypto"
import { EventEmitter } from "node:events"

// Tauri 2.12.4's path initializer also runs in Windows child frames. The
// framework's IPC globals can be absent there. Recognize only that exact
// initializer in a verified child frame; application/renderer errors still fail.
const normalize = (source) =>
  source.replace(/\/\/[^\r\n]*/g, "").replace(/\s+/g, "")
const pathInitializer = normalize(`(function () {
  Object.defineProperty(window.__TAURI_INTERNALS__.plugins, 'path', {
    value: { sep: ${JSON.stringify("\\")}, delimiter: ";" }
  })
})();`)

export function isEmbeddedTauriPathError(error) {
  return (
    error.mainFrame === false &&
    error.message ===
      "TypeError: Cannot read properties of undefined (reading 'plugins')" &&
    typeof error.source === "string" &&
    normalize(error.source) === pathInitializer
  )
}

export function correlateRuntimeErrors(rawErrors, exceptions) {
  const remaining = [...rawErrors]
  const embeddedFrameworkErrors = []
  const errors = []
  for (const exception of exceptions) {
    const index = remaining.indexOf(exception.message)
    if (index >= 0) remaining.splice(index, 1)
    const detail = { ...exception }
    delete detail.source
    if (isEmbeddedTauriPathError(exception))
      embeddedFrameworkErrors.push(detail)
    else errors.push(detail)
  }
  // A missing CDP observation cannot turn a Playwright error into a pass.
  errors.push(...remaining.map((message) => ({ message, uncorrelated: true })))
  return { errors, rawErrors, embeddedFrameworkErrors }
}

// Use a separate CDP attachment so Playwright keeps its own frame lifecycle.
// Non-flat child sessions let us observe out-of-process iframes before their
// initialization scripts run, without depending on Playwright private fields.
function childSession(parent, sessionId) {
  const session = new EventEmitter()
  const requests = new Map()
  let nextId = 0
  const received = (event) => {
    if (event.sessionId !== sessionId) return
    const packet = JSON.parse(event.message)
    if (packet.id) {
      const request = requests.get(packet.id)
      if (!request) return
      requests.delete(packet.id)
      clearTimeout(request.timer)
      if (packet.error) request.reject(Error(packet.error.message))
      else request.resolve(packet.result)
    } else session.emit(packet.method, packet.params)
  }
  parent.on("Target.receivedMessageFromTarget", received)
  session.send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++nextId
      const timer = setTimeout(() => {
        requests.delete(id)
        reject(Error(`Child CDP ${method} timed out`))
      }, 10000)
      requests.set(id, { resolve, reject, timer })
      parent
        .send("Target.sendMessageToTarget", {
          sessionId,
          message: JSON.stringify({ id, method, params }),
        })
        .catch((error) => {
          requests.delete(id)
          clearTimeout(timer)
          reject(error)
        })
    })
  session.detach = async () => {
    await parent.send("Target.detachFromTarget", { sessionId }).catch(() => {})
    parent.off("Target.receivedMessageFromTarget", received)
  }
  return session
}

export async function scopedRuntimeErrors(page) {
  const rawErrors = []
  const exceptions = []
  const pending = []
  const inspectionErrors = []
  const cleanups = []
  const children = []
  const initializing = []
  let stopping = false
  const client = await page.context().newCDPSession(page)
  const { frameTree } = await client.send("Page.getFrameTree")
  const mainFrameId = frameTree.frame.id
  const listener = (error) => rawErrors.push(String(error))
  const attachPolicy = {
    autoAttach: true,
    waitForDebuggerOnStart: true,
    flatten: false,
  }
  function inspect(target, knownFrameId) {
    const contexts = new Map()
    const contextCreated = ({ context }) => contexts.set(context.id, context)
    const thrown = ({ exceptionDetails: detail }) => {
      const context = contexts.get(detail.executionContextId)
      const frameId = context?.auxData?.frameId ?? knownFrameId
      const scriptId = detail.stackTrace?.callFrames?.[0]?.scriptId
      const message = (detail.exception?.description ?? detail.text).split(
        "\n"
      )[0]
      pending.push(
        (async () => {
          const script = scriptId
            ? await target
                .send("Debugger.getScriptSource", { scriptId })
                .catch(() => null)
            : null
          const source = script?.scriptSource ?? ""
          let origin
          try {
            origin = new URL(context.origin).origin
          } catch {}
          exceptions.push({
            message,
            mainFrame: frameId ? frameId === mainFrameId : null,
            frameId,
            origin,
            source,
            sourceSha256: source
              ? createHash("sha256").update(source).digest("hex")
              : null,
            stack: detail.stackTrace,
          })
        })()
      )
    }
    const attached = ({ sessionId, targetInfo }) => {
      const child = childSession(target, sessionId)
      children.push(child)
      initializing.push(
        (async () => {
          try {
            if (!stopping && targetInfo.type === "iframe") {
              inspect(child, targetInfo.targetId)
              await child.send("Debugger.enable")
              await child.send("Runtime.enable")
              await child.send("Target.setAutoAttach", attachPolicy)
            }
          } catch (error) {
            inspectionErrors.push({ message: String(error), inspector: true })
          } finally {
            await child
              .send("Runtime.runIfWaitingForDebugger")
              .catch((error) => {
                inspectionErrors.push({
                  message: String(error),
                  inspector: true,
                })
              })
          }
        })()
      )
    }
    target.on("Runtime.executionContextCreated", contextCreated)
    target.on("Runtime.exceptionThrown", thrown)
    target.on("Target.attachedToTarget", attached)
    cleanups.push(() => {
      target.off("Runtime.executionContextCreated", contextCreated)
      target.off("Runtime.exceptionThrown", thrown)
      target.off("Target.attachedToTarget", attached)
    })
  }
  page.on("pageerror", listener)
  inspect(client)
  await client.send("Debugger.enable")
  await client.send("Runtime.enable")
  await client.send("Target.setAutoAttach", attachPolicy)
  let stopped
  return {
    stop() {
      stopped ??= (async () => {
        stopping = true
        await Promise.all(initializing)
        await client.send("Target.setAutoAttach", {
          ...attachPolicy,
          autoAttach: false,
          waitForDebuggerOnStart: false,
        })
        page.off("pageerror", listener)
        cleanups.forEach((cleanup) => cleanup())
        await Promise.all(pending)
        for (const child of [...children].reverse()) await child.detach()
        await client.detach()
        const result = correlateRuntimeErrors([...rawErrors], exceptions)
        result.errors.push(...inspectionErrors)
        return result
      })()
      return stopped
    },
  }
}
