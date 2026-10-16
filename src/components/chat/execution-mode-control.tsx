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
  const { connectionId, readOnly } = useGoalControl()
  // Keying the inner component makes connection changes discard pending UI.
  return (
    <ExecutionModeSession
      key={connectionId ?? "offline"}
      option={option}
      connectionId={connectionId ?? null}
      readOnly={Boolean(readOnly)}
      onSelect={onSelect}
      active={active}
    />
  )
}

function ExecutionModeSession({
  option,
  connectionId,
  readOnly,
  onSelect,
  active,
}: {
  active: boolean
  option: SessionConfigOptionInfo
  connectionId: string | null
  readOnly: boolean
  onSelect?: (configId: string, value: string) => void
}) {
  const t = useTranslations("Folder.chat.execution")
  const [snapshot, setSnapshot] = useState<SessionGoalState | null>(null)
  const [goalSelected, setGoalSelected] = useState(false)
  const [objective, setObjective] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const mounted = useRef(true)
  const refresh = useCallback(async () => {
    if (!connectionId) return null
    const next = await acpGetGoalState(connectionId)
    if (mounted.current) setSnapshot(next)
    return next
  }, [connectionId])

  useEffect(() => {
    mounted.current = true
    if (!active)
      return () => {
        mounted.current = false
      }
    let timer: ReturnType<typeof setTimeout>
    // A small projection also recovers externally-created/restored goals.
    // No transcript polling, overlapping requests, or background-tab traffic.
    const read = async () => {
      try {
        if (document.visibilityState !== "hidden") await refresh()
      } catch {
        if (mounted.current) setSnapshot(null)
      }
      if (mounted.current) timer = setTimeout(read, 1500)
    }
    void read()
    return () => {
      mounted.current = false
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
  const disabled = busy || readOnly || !connectionId

  const run = async (action: "set" | "pause" | "resume" | "clear") => {
    if (!connectionId || disabled) return
    setBusy(true)
    setError(null)
    try {
      await acpGoalControl(
        connectionId,
        action,
        action === "set" ? objective.trim() : undefined
      )
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
