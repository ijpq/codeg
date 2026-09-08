"use client"

import { useCallback, useEffect, useState } from "react"
import { Gauge, RefreshCw } from "lucide-react"
import { useTranslations } from "next-intl"
import { codexQuotaSnapshot } from "@/lib/api"
import type { AgentType, CodexQuotaSnapshot } from "@/lib/types"
import { useTabStore } from "@/contexts/tab-context"
import {
  codexPlanLabel,
  formatQuotaPercent,
  quotaRemainingPercent,
} from "@/lib/codex-quota"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"

const IDLE_REFRESH_MS = 5 * 60 * 1000

interface ComposerCodexQuotaProps {
  tabId: string | null
  agentType: AgentType | null | undefined
  isPrompting: boolean
}

/**
 * Compact package + weekly allowance badge beside the model selector.
 *
 * The fetch reads Codex's local rollout only; it does not spend tokens or call
 * the relay. A turn transition gives near-immediate freshness, while the
 * five-minute timer covers changes made by another Codeg/browser client.
 */
export function ComposerCodexQuota({
  tabId,
  agentType,
  isPrompting,
}: ComposerCodexQuotaProps) {
  const t = useTranslations("Folder.statusBar.quota")
  const conversationId = useTabStore((state) => {
    const tab = state.tabs.find((item) => item.id === tabId)
    if (!tab || tab.kind !== "conversation") return null
    const resolved = tab.runtimeConversationId ?? tab.conversationId ?? null
    return resolved != null && resolved > 0 ? resolved : null
  })
  const [snapshot, setSnapshot] = useState<CodexQuotaSnapshot | null>(null)
  const [refreshing, setRefreshing] = useState(false)

  const refresh = useCallback(async () => {
    if (agentType !== "codex") {
      setSnapshot(null)
      return
    }
    setRefreshing(true)
    try {
      const next = await codexQuotaSnapshot(conversationId)
      setSnapshot(next)
    } catch {
      // Quota is optional status chrome. Keep the last good observation on a
      // transient local read/API failure instead of distracting with a toast.
    } finally {
      setRefreshing(false)
    }
  }, [agentType, conversationId])

  useEffect(() => {
    if (agentType !== "codex") {
      setSnapshot(null)
      return
    }
    // `isPrompting` is intentionally a dependency: entering a turn preserves
    // the previous reading; leaving it picks up the response's new rate_limits.
    void refresh()
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void refresh()
    }, IDLE_REFRESH_MS)
    return () => window.clearInterval(timer)
  }, [agentType, isPrompting, refresh])

  if (agentType !== "codex" || !snapshot?.weekly) return null

  const plan = codexPlanLabel(snapshot)
  const remaining = quotaRemainingPercent(snapshot.weekly.usedPercent)
  const remainingLabel = formatQuotaPercent(remaining)
  const usedLabel = formatQuotaPercent(snapshot.weekly.usedPercent)
  const resetLabel = snapshot.weekly.resetsAt
    ? new Date(snapshot.weekly.resetsAt * 1000).toLocaleString()
    : t("unknown")
  const observedLabel = snapshot.observedAt
    ? new Date(snapshot.observedAt).toLocaleString()
    : t("unknown")
  const title = t("summary", { plan, remaining: remainingLabel })

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          title={title}
          aria-label={title}
          className="flex h-7 shrink-0 items-center gap-1 rounded-md px-1.5 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground"
        >
          <Gauge className="size-3.5" />
          <span className="max-w-24 truncate">{plan}</span>
          <span className="whitespace-nowrap tabular-nums">
            {t("weeklyShort")} {remainingLabel}
          </span>
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" className="w-64 p-3 text-xs">
        <div className="flex items-center justify-between gap-2">
          <div>
            <div className="font-medium">{plan}</div>
            <div className="text-muted-foreground">{t("lastRoutedPlan")}</div>
          </div>
          <button
            type="button"
            onClick={() => void refresh()}
            disabled={refreshing}
            title={t("refresh")}
            aria-label={t("refresh")}
            className="rounded-md p-1.5 text-muted-foreground hover:bg-accent hover:text-accent-foreground disabled:opacity-50"
          >
            <RefreshCw
              className={`size-3.5 ${refreshing ? "animate-spin" : ""}`}
            />
          </button>
        </div>
        <div className="mt-3 space-y-1.5">
          <div className="flex items-center justify-between gap-2">
            <span className="text-muted-foreground">
              {t("weeklyRemaining")}
            </span>
            <span className="font-medium tabular-nums">{remainingLabel}</span>
          </div>
          <div className="h-1.5 overflow-hidden rounded-full bg-muted">
            <div
              className="h-full rounded-full bg-foreground/70"
              style={{ width: `${remaining}%` }}
            />
          </div>
          <div className="flex items-center justify-between gap-2 text-muted-foreground">
            <span>{t("weeklyUsed")}</span>
            <span className="tabular-nums">{usedLabel}</span>
          </div>
          <div className="flex items-center justify-between gap-2 text-muted-foreground">
            <span>{t("resetsAt")}</span>
            <span className="truncate text-right">{resetLabel}</span>
          </div>
          <div className="flex items-center justify-between gap-2 text-muted-foreground">
            <span>{t("observedAt")}</span>
            <span className="truncate text-right">{observedLabel}</span>
          </div>
        </div>
        <p className="mt-2 border-t pt-2 text-muted-foreground">
          {t("sourceHint")}
        </p>
      </PopoverContent>
    </Popover>
  )
}
