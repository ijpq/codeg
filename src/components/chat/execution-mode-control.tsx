"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { ChevronDown } from "lucide-react"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import { Textarea } from "@/components/ui/textarea"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import { useGoalControl } from "@/components/message/goal-control-context"
import {
  acpGetGoalState,
  acpGoalControl,
  type SessionGoalState,
} from "@/lib/api"
import type { SessionConfigOptionInfo } from "@/lib/types"
import { formatElapsedLabel } from "@/lib/format-elapsed"

const GOAL_STATUS_KEYS = {
  active: "statuses.active",
  paused: "statuses.paused",
  blocked: "statuses.blocked",
  complete: "statuses.complete",
  limited: "statuses.limited",
  budgetLimited: "statuses.budgetLimited",
  usageLimited: "statuses.usageLimited",
} as const

/** The three choices share one native collaboration selector. Goal is a
 * separate ACP capability, never a collaboration_mode value or a prompt. */
export function ExecutionModeControl({
  option,
  onSelect,
  active = true,
}: {
  active?: boolean
  option: SessionConfigOptionInfo
  onSelect?: (configId: string, value: string) => void
}) {
  const { connectionId, readOnly, startGoal, onGoalSnapshot } = useGoalControl()
  // Keying the inner component makes connection changes discard pending UI.
  return (
    <ExecutionModeSession
      key={connectionId ?? "offline"}
      option={option}
      connectionId={connectionId ?? null}
      readOnly={Boolean(readOnly)}
      startGoal={startGoal}
      onGoalSnapshot={onGoalSnapshot}
      onSelect={onSelect}
      active={active}
    />
  )
}

function ExecutionModeSession({
  option,
  connectionId,
  readOnly,
  startGoal,
  onGoalSnapshot,
  onSelect,
  active,
}: {
  active: boolean
  option: SessionConfigOptionInfo
  connectionId: string | null
  readOnly: boolean
  startGoal?: (objective: string) => Promise<void>
  onGoalSnapshot?: (snapshot: SessionGoalState) => void
  onSelect?: (configId: string, value: string) => void
}) {
  const t = useTranslations("Folder.chat.execution")
  const tElapsed = useTranslations("Folder.chat.liveTurnStats")
  const tGoal = useTranslations("Folder.chat.contentParts.goal")
  const [snapshot, setSnapshot] = useState<SessionGoalState | null>(null)
  const [goalSelected, setGoalSelected] = useState(false)
  const [objective, setObjective] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [stateUnavailable, setStateUnavailable] = useState(false)
  const mounted = useRef(true)
  const readSequence = useRef(0)
  const refresh = useCallback(async () => {
    if (!connectionId) return null
    const sequence = ++readSequence.current
    try {
      const next = await acpGetGoalState(connectionId)
      if (mounted.current && sequence === readSequence.current) {
        setSnapshot(next)
        setStateUnavailable(false)
        onGoalSnapshot?.(next)
      }
      return next
    } catch (error) {
      if (mounted.current && sequence === readSequence.current)
        setStateUnavailable(true)
      throw error
    }
  }, [connectionId, onGoalSnapshot])

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  useEffect(() => {
    if (!active) return
    let cancelled = false
    let timer: ReturnType<typeof setTimeout>
    // A small projection also recovers externally-created/restored goals.
    // Status reads do not overlap or run in background tabs.
    const read = async () => {
      try {
        if (document.visibilityState !== "hidden") await refresh()
      } catch {
        // Keep the last known goal visible; a failed read never means cleared.
      }
      if (!cancelled) timer = setTimeout(read, 1500)
    }
    void read()
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [refresh, active])

  const goal = snapshot?.goal
  const actions = snapshot?.actions ?? []
  const supported = ["set", "pause", "resume", "clear"].every((action) =>
    actions.includes(action)
  )
  const nativeMode =
    option.kind.type === "select" ? option.kind.current_value : "default"
  const mode =
    goal?.status === "active" || goalSelected
      ? "goal"
      : nativeMode === "plan"
        ? "plan"
        : "direct"
  const disabled = busy || readOnly || !connectionId || stateUnavailable

  const run = async (action: "set" | "pause" | "resume" | "clear") => {
    if (!connectionId || disabled) return
    setBusy(true)
    setError(null)
    try {
      if (action === "set" && startGoal) {
        await startGoal(objective.trim())
      } else {
        await acpGoalControl(
          connectionId,
          action,
          action === "set" ? objective.trim() : undefined
        )
      }
      await refresh()
    } catch (e) {
      if (mounted.current) setError(String(e))
    } finally {
      if (mounted.current) setBusy(false)
    }
  }

  const selectMode = async (next: string) => {
    if (disabled) return
    if (next === "goal") {
      setGoalSelected(true)
      return
    }
    setBusy(true)
    setError(null)
    try {
      // Read immediately: another client or the CLI may have armed a goal
      // since the last poll. Failure leaves this selector and the goal intact.
      const current = await refresh()
      if (!current) throw new Error(t("stateUnavailable"))
      if (current.goal?.status === "active") {
        if (!current.actions?.includes("pause"))
          throw new Error(t("unsupported"))
        await acpGoalControl(connectionId!, "pause")
        await refresh()
      }
      if (mounted.current) {
        onSelect?.(option.id, next === "plan" ? "plan" : "default")
        setGoalSelected(false)
      }
    } catch (e) {
      if (mounted.current) setError(String(e))
    } finally {
      if (mounted.current) setBusy(false)
    }
  }

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="xs"
          className="gap-0.5 px-1 text-muted-foreground"
          aria-label={`${t("label")}: ${t(mode)}`}
        >
          {t(mode)}
          <ChevronDown className="size-3" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        side="top"
        align="start"
        className="w-80 max-w-[calc(100vw-1rem)] space-y-3"
      >
        <div role="radiogroup" aria-label={t("label")} className="flex gap-1">
          {(["direct", "plan", "goal"] as const).map((value) => (
            <Button
              key={value}
              role="radio"
              aria-checked={mode === value}
              variant={mode === value ? "secondary" : "ghost"}
              size="sm"
              disabled={disabled || (value === "goal" && !supported)}
              onClick={() => void selectMode(value)}
            >
              {t(value)}
            </Button>
          ))}
        </div>
        <p className="text-xs text-muted-foreground">{t("switchHint")}</p>
        {stateUnavailable && (
          <p role="status" className="text-xs text-destructive">
            {t("stateUnavailable")}
          </p>
        )}
        {!supported && (
          <p role="status" className="text-xs text-muted-foreground">
            {t("unsupported")}
          </p>
        )}
        {(mode === "goal" || goal) && (
          <div className="space-y-2">
            {goal ? (
              <>
                <p className="max-h-32 overflow-y-auto whitespace-pre-wrap break-words text-sm">
                  {goal.objective}
                </p>
                <p role="status" className="text-xs">
                  {t("status")}:{" "}
                  {GOAL_STATUS_KEYS[
                    goal.status as keyof typeof GOAL_STATUS_KEYS
                  ]
                    ? t(
                        GOAL_STATUS_KEYS[
                          goal.status as keyof typeof GOAL_STATUS_KEYS
                        ]
                      )
                    : goal.status}
                </p>
                {goal.tokensUsed != null && (
                  <p className="text-xs text-muted-foreground">
                    {t("tokens")}: {goal.tokensUsed}
                    {goal.tokenBudget != null ? ` / ${goal.tokenBudget}` : ""}
                  </p>
                )}
                {typeof goal.timeUsedSeconds === "number" &&
                  Number.isFinite(goal.timeUsedSeconds) &&
                  goal.timeUsedSeconds >= 0 && (
                    <p
                      className="text-xs tabular-nums text-muted-foreground"
                      data-goal-duration={goal.timeUsedSeconds}
                    >
                      {tGoal("elapsed")}:{" "}
                      {formatElapsedLabel(
                        goal.timeUsedSeconds * 1000,
                        tElapsed
                      )}
                    </p>
                  )}
                <div className="flex gap-2">
                  {goal.status === "active" ? (
                    <Button
                      size="sm"
                      disabled={disabled || !actions.includes("pause")}
                      onClick={() => void run("pause")}
                    >
                      {t("pause")}
                    </Button>
                  ) : (
                    goal.status !== "complete" && (
                      <Button
                        size="sm"
                        disabled={disabled || !supported}
                        onClick={() => void run("resume")}
                      >
                        {t("resume")}
                      </Button>
                    )
                  )}
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={disabled || !actions.includes("clear")}
                    onClick={() => void run("clear")}
                  >
                    {t("end")}
                  </Button>
                </div>
              </>
            ) : (
              <>
                <Textarea
                  aria-label={t("objective")}
                  placeholder={t("objective")}
                  value={objective}
                  onChange={(event) => setObjective(event.target.value)}
                  disabled={disabled}
                  className="max-h-40"
                />
                <Button
                  size="sm"
                  disabled={disabled || !supported || !objective.trim()}
                  onClick={() => void run("set")}
                >
                  {t("start")}
                </Button>
              </>
            )}
          </div>
        )}
        {error && (
          <p role="alert" className="break-words text-xs text-destructive">
            {error}
          </p>
        )}
      </PopoverContent>
    </Popover>
  )
}
