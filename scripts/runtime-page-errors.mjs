import { createHash } from "node:crypto"

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

export async function scopedRuntimeErrors(page) {
  const rawErrors = []
  const exceptions = []
  const contexts = new Map()
  const pending = []
  const client = await page.context().newCDPSession(page)
  const { frameTree } = await client.send("Page.getFrameTree")
  const mainFrameId = frameTree.frame.id
  const listener = (error) => rawErrors.push(String(error))
  const contextCreated = ({ context }) => contexts.set(context.id, context)
  const thrown = ({ exceptionDetails: detail }) => {
    const context = contexts.get(detail.executionContextId)
    const frameId = context?.auxData?.frameId
    const scriptId = detail.stackTrace?.callFrames?.[0]?.scriptId
    const message = (detail.exception?.description ?? detail.text).split(
      "\n"
    )[0]
    pending.push(
      (async () => {
        const script = scriptId
          ? await client
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
  page.on("pageerror", listener)
  client.on("Runtime.executionContextCreated", contextCreated)
  client.on("Runtime.exceptionThrown", thrown)
  await client.send("Debugger.enable")
  await client.send("Runtime.enable")
  let stopped
  return {
    stop() {
      stopped ??= (async () => {
        page.off("pageerror", listener)
        client.off("Runtime.exceptionThrown", thrown)
        client.off("Runtime.executionContextCreated", contextCreated)
        await Promise.all(pending)
        await client.detach()
        return correlateRuntimeErrors([...rawErrors], exceptions)
      })()
      return stopped
    },
  }
}
