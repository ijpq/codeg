import { appendFileSync, writeFileSync } from "node:fs"
import { join } from "node:path"

// Labels contain operation names only, never API arguments, tokens or prompts.
export function verificationProgress(output, report) {
  const active = new Map()
  let sequence = 0
  function record(event, label) {
    const value = {
      time: new Date().toISOString(),
      event,
      label,
      completedChecks: report.checks.map((check) => check.name),
      active: [...active.values()],
    }
    writeFileSync(
      join(output, "verification-progress.json"),
      JSON.stringify(value, null, 2)
    )
    appendFileSync(
      join(output, "verification-progress.log"),
      JSON.stringify(value) + "\n"
    )
    if (event !== "begin" && event !== "end")
      console.log(
        JSON.stringify({
          time: value.time,
          event,
          label,
          lastCheck: value.completedChecks.at(-1),
          active: value.active,
        })
      )
  }
  return {
    record,
    async run(label, action, timeoutMs = 120_000) {
      const id = ++sequence
      active.set(id, { label, startedAt: new Date().toISOString() })
      record("begin", label)
      let timer
      try {
        return await Promise.race([
          Promise.resolve().then(action),
          new Promise((_, reject) => {
            timer = setTimeout(
              () => reject(Error(`${label} timed out after ${timeoutMs} ms`)),
              timeoutMs
            )
          }),
        ])
      } catch (error) {
        record("failed", label)
        throw error
      } finally {
        clearTimeout(timer)
        active.delete(id)
        record("end", label)
      }
    },
  }
}
