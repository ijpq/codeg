"use client"

import { Clock } from "lucide-react"
import { useTranslations } from "next-intl"
import { formatElapsedLabel } from "@/lib/format-elapsed"
import { taskRunDurationMs } from "@/lib/task-duration"
import type { WorkTask } from "@/lib/types"

/** One value for cards, rows and details; no timer runs for settled tasks. */
export function TaskDuration({ task }: { task: WorkTask }) {
  const t = useTranslations("Tasks")
  const tElapsed = useTranslations("Folder.chat.liveTurnStats")
  const ms = taskRunDurationMs(task)
  if (ms === null) return null
  const label = t("runDuration", { duration: formatElapsedLabel(ms, tElapsed) })
  return (
    <span
      className="inline-flex min-w-0 items-center gap-1 text-[0.6875rem] tabular-nums text-muted-foreground"
      title={t("runDurationHint")}
      data-task-duration={task.id}
    >
      <Clock className="size-3 shrink-0" aria-hidden="true" />
      <span className="truncate">{label}</span>
    </span>
  )
}
