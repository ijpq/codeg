import type { WorkTask } from "@/lib/types"

/** The most recent work round, measured by persisted engine timestamps.
 * `started_at` is replaced by mark_running; `settled_at` ends the work before
 * review/acceptance/merge. Live generations can still carry older timestamps.
 */
export function taskRunDurationMs(task: WorkTask): number | null {
  // Setup failures and cancellation can still carry a previous generation's
  // start. Only reviewed/accepted work guarantees these timestamps pair up.
  if (!["review", "done"].includes(task.status)) return null
  if (!task.started_at || !task.settled_at) return null
  const start = Date.parse(task.started_at)
  const end = Date.parse(task.settled_at)
  if (!Number.isFinite(start) || !Number.isFinite(end) || end < start)
    return null
  return end - start
}
