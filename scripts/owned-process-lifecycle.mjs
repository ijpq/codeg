import { appendFileSync } from "node:fs"
import { join } from "node:path"

const observed = new WeakMap()

// Only fixed harness labels and OS process metadata belong here. In particular,
// never serialize spawn arguments, environment, stdout or an Error's message.
export function observeOwnedProcess(child, { output, name }) {
  const state = { name, startedAt: new Date().toISOString(), stopReason: null }
  const record = (event, details = {}) => {
    const value = {
      time: new Date().toISOString(),
      event,
      name,
      pid: child.pid ?? null,
      startedAt: state.startedAt,
      stopReason: state.stopReason,
      ...details,
    }
    appendFileSync(
      join(output, "owned-process-lifecycle.log"),
      JSON.stringify(value) + "\n"
    )
    console.log(JSON.stringify(value))
  }
  observed.set(child, { state, record })
  record("spawn")
  child.on("error", (error) =>
    record("spawn-error", { code: error.code ?? null })
  )
  child.on("exit", (code, signal) => record("exit", { code, signal }))
  child.on("close", (code, signal) => record("close", { code, signal }))
  return state
}

export function markOwnedProcessStop(child, reason) {
  const entry = observed.get(child)
  if (!entry) return
  // A finally block after a crash must not reclassify that crash as a requested
  // stop. Record the observed status even if the exit event has not run yet.
  if (child.exitCode !== null || child.signalCode !== null) {
    entry.record("stop-already-exited", {
      code: child.exitCode,
      signal: child.signalCode,
      requestedReason: reason,
    })
    return
  }
  entry.state.stopReason ??= reason
  entry.record("stop-requested")
}

export function captureOwnedProcessLog(child, log) {
  // Ending stdout does not mean stderr has drained (and vice versa). In
  // particular a fatal runtime error can be the last write to stderr.
  child.stdout.pipe(log, { end: false })
  child.stderr.pipe(log, { end: false })
  let closeLog
  const drained = new Promise((resolve) => {
    closeLog = () => log.end(resolve)
    child.once("close", closeLog)
  })
  return async () => {
    let timer
    try {
      await Promise.race([
        drained,
        new Promise((resolve) => {
          timer = setTimeout(() => {
            observed.get(child)?.record("output-drain-timeout")
            child.removeListener("close", closeLog)
            child.stdout.unpipe(log)
            child.stderr.unpipe(log)
            log.end(resolve)
          }, 3000)
        }),
      ])
    } finally {
      clearTimeout(timer)
    }
  }
}
