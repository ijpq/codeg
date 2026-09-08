"use client"

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react"
import {
  Archive,
  ChevronDownIcon,
  ClipboardCopy,
  Download,
  FileIcon,
  FolderIcon,
  FolderSearch,
  PackageCheck,
  Loader2,
  RefreshCw,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { CollapsedOverlayChip } from "@/components/chat/collapsed-overlay-chip"
import { DeliverableFileActions } from "@/components/message/deliverable-file-actions"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import { useDeliverableCapabilities } from "@/hooks/use-deliverable-capabilities"
import {
  copyDeliverableFiles,
  downloadDeliverables,
  listConversationDeliverableHistory,
  revealDeliverable,
} from "@/lib/api"
import { subscribe } from "@/lib/platform"
import type {
  ConversationDeliverableHistoryGroup,
  ConversationDeliverablesChanged,
} from "@/lib/types"
import { CONVERSATION_DELIVERABLES_CHANGED_EVENT } from "@/lib/types"

interface ConversationDeliverablesPanelProps {
  conversationId: number
  expanded: boolean
  onToggle: (next: boolean) => void
}

const HISTORY_PAGE_SIZE = 25

function formatBytes(value?: number | null): string {
  if (value == null) return "—"
  if (value < 1024) return `${value}B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)}KB`
  if (value < 1024 * 1024 * 1024) {
    return `${(value / (1024 * 1024)).toFixed(1)}MB`
  }
  return `${(value / (1024 * 1024 * 1024)).toFixed(1)}GB`
}

function formatCompactTimestamp(value: string): string {
  const date = new Date(value)
  const minute = String(date.getMinutes()).padStart(2, "0")
  return `${date.getMonth() + 1}.${date.getDate()} ${date.getHours()}:${minute}`
}

export const ConversationDeliverablesPanel = memo(
  function ConversationDeliverablesPanel({
    conversationId,
    expanded,
    onToggle,
  }: ConversationDeliverablesPanelProps) {
    const t = useTranslations("Folder.chat.conversationDeliverables")
    const capabilities = useDeliverableCapabilities()
    const [typeFilter, setTypeFilter] = useState("all")
    const [sourceFilter, setSourceFilter] = useState("all")
    const [turnFilter, setTurnFilter] = useState("all")
    const [timeFilter, setTimeFilter] = useState("all")
    const [timeCutoff, setTimeCutoff] = useState<number | null>(null)
    const [sort, setSort] = useState("newest")
    const [selected, setSelected] = useState<Set<string>>(() => new Set())
    const [groups, setGroups] = useState<ConversationDeliverableHistoryGroup[]>(
      []
    )
    const [total, setTotal] = useState(0)
    const [nextOffset, setNextOffset] = useState<number | null>(null)
    const [loading, setLoading] = useState(false)
    const [loaded, setLoaded] = useState(false)
    const [error, setError] = useState<string | null>(null)
    const [reloadVersion, setReloadVersion] = useState(0)
    const requestGeneration = useRef(0)
    const visible = useMemo(() => groups.map((group) => group.latest), [groups])
    const groupById = useMemo(
      () => new Map(groups.map((group) => [group.latest.id, group])),
      [groups]
    )

    const loadPage = useCallback(
      async (offset: number, append: boolean) => {
        const generation = ++requestGeneration.current
        setLoading(true)
        setError(null)
        try {
          const page = await listConversationDeliverableHistory(
            conversationId,
            offset,
            HISTORY_PAGE_SIZE
          )
          if (generation !== requestGeneration.current) return
          setGroups((previous) =>
            append ? [...previous, ...page.items] : page.items
          )
          setTotal(page.total)
          setNextOffset(page.has_more ? (page.next_offset ?? null) : null)
          setLoaded(true)
        } catch (loadError) {
          if (generation !== requestGeneration.current) return
          console.error(
            "[ConversationDeliverables] failed to load history",
            loadError
          )
          setError(
            loadError instanceof Error ? loadError.message : String(loadError)
          )
        } finally {
          if (generation === requestGeneration.current) setLoading(false)
        }
      },
      [conversationId]
    )

    useEffect(() => {
      requestGeneration.current += 1
      setGroups([])
      setTotal(0)
      setNextOffset(null)
      setLoaded(false)
      setError(null)
      setSelected(new Set())
      return () => {
        requestGeneration.current += 1
      }
    }, [conversationId])

    useEffect(() => {
      if (!expanded) return
      void loadPage(0, false)
    }, [expanded, loadPage, reloadVersion])

    useEffect(() => {
      let disposed = false
      let unlisten: (() => void) | undefined
      void subscribe<ConversationDeliverablesChanged>(
        CONVERSATION_DELIVERABLES_CHANGED_EVENT,
        (change) => {
          if (change.conversation_id === conversationId) {
            setLoaded(false)
            setReloadVersion((previous) => previous + 1)
          }
        }
      ).then((dispose) => {
        if (disposed) dispose()
        else unlisten = dispose
      })
      return () => {
        disposed = true
        unlisten?.()
      }
    }, [conversationId])
    const extensions = useMemo(
      () =>
        [...new Set(visible.map((item) => item.extension ?? item.kind))].sort(),
      [visible]
    )
    const turnIds = useMemo(() => {
      const ids = new Set<string>()
      groups.forEach((group) =>
        group.versions.forEach((item) => {
          if (item.turn_run_id) ids.add(item.turn_run_id)
        })
      )
      return [...ids]
    }, [groups])
    const filtered = useMemo(() => {
      const rows = visible.filter((item) => {
        const type = item.extension ?? item.kind
        const withinTime =
          timeFilter === "all" ||
          (timeCutoff !== null &&
            new Date(item.updated_at).getTime() >= timeCutoff)
        return (
          (typeFilter === "all" || type === typeFilter) &&
          (sourceFilter === "all" || item.source === sourceFilter) &&
          (turnFilter === "all" || item.turn_run_id === turnFilter) &&
          withinTime
        )
      })
      return rows.sort((left, right) => {
        const delta =
          new Date(left.updated_at).getTime() -
          new Date(right.updated_at).getTime()
        return sort === "oldest" ? delta : -delta
      })
    }, [
      sourceFilter,
      sort,
      timeCutoff,
      timeFilter,
      turnFilter,
      typeFilter,
      visible,
    ])

    const changeTimeFilter = (value: string) => {
      setTimeFilter(value)
      const days = value === "day" ? 1 : value === "week" ? 7 : 30
      setTimeCutoff(
        value === "all" ? null : Date.now() - days * 24 * 60 * 60 * 1000
      )
    }

    if (!expanded) {
      return (
        <CollapsedOverlayChip
          icon={<PackageCheck className="size-3.5" />}
          summary={
            loaded ? t("collapsedSummary", { count: total }) : t("historyTitle")
          }
          onClick={() => onToggle(true)}
        />
      )
    }

    const selectedItems = visible.filter((item) => selected.has(item.id))
    const validSelected = selectedItems.filter((item) => item.is_valid)
    const allFilteredSelected =
      filtered.length > 0 && filtered.every((item) => selected.has(item.id))
    const toggleAll = (checked: boolean) => {
      setSelected((previous) => {
        const next = new Set(previous)
        filtered.forEach((item) =>
          checked ? next.add(item.id) : next.delete(item.id)
        )
        return next
      })
    }
    const runBatch = async (
      operation: () => Promise<unknown>,
      success: string
    ) => {
      try {
        await operation()
        toast.success(success)
      } catch (error) {
        console.error(
          "[ConversationDeliverables] batch operation failed",
          error
        )
        toast.error(t("operationFailed"))
      }
    }
    const batchActions = [
      {
        id: "download",
        label: t("downloadSelected"),
        icon: Download,
        enabled: validSelected.length > 0,
        action: () =>
          runBatch(async () => {
            for (const item of validSelected) {
              await downloadDeliverables({
                conversationId,
                deliverableIds: [item.id],
                archive: item.kind === "directory",
                suggestedName:
                  item.kind === "directory"
                    ? `${item.file_name}.zip`
                    : item.file_name,
              })
            }
          }, t("downloadStarted")),
      },
      {
        id: "zip",
        label: t("downloadZip"),
        icon: Archive,
        enabled: validSelected.length > 0,
        action: () =>
          runBatch(
            () =>
              downloadDeliverables({
                conversationId,
                deliverableIds: validSelected.map((item) => item.id),
                archive: true,
              }),
            t("downloadStarted")
          ),
      },
      {
        id: "copy",
        label: t("copySelectedHost"),
        icon: ClipboardCopy,
        enabled: validSelected.length > 0 && capabilities?.copyFiles === true,
        action: () =>
          runBatch(
            () =>
              copyDeliverableFiles(
                conversationId,
                validSelected.map((item) => item.id)
              ),
            t("filesCopied")
          ),
      },
      {
        id: "reveal",
        label: t("revealFirstHost"),
        icon: FolderSearch,
        enabled:
          validSelected.length > 0 && capabilities?.revealInFolder === true,
        action: () =>
          runBatch(
            () => revealDeliverable(conversationId, validSelected[0].id),
            t("revealed")
          ),
      },
    ]

    return (
      <div className="pointer-events-none flex w-[29rem] max-w-[calc(100vw-2rem)]">
        <div className="pointer-events-auto w-full overflow-hidden rounded-xl border bg-card/90 shadow-lg backdrop-blur">
          <div className="flex items-center justify-between border-b px-3 py-2">
            <div className="flex min-w-0 items-center gap-2">
              <PackageCheck className="size-4 text-muted-foreground" />
              <span className="truncate text-sm font-medium">
                {t("historyTitle")}
              </span>
              <Badge variant="secondary" className="h-5">
                {total}
              </Badge>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              aria-label={t("collapse")}
              onClick={() => onToggle(false)}
            >
              <ChevronDownIcon className="size-4" />
            </Button>
          </div>

          <div className="flex flex-wrap gap-1.5 border-b p-2">
            <Select value={typeFilter} onValueChange={setTypeFilter}>
              <SelectTrigger
                size="sm"
                className="h-7 max-w-28 px-2 text-[11px]"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">{t("allTypes")}</SelectItem>
                {extensions.map((extension) => (
                  <SelectItem key={extension} value={extension}>
                    {extension.toUpperCase()}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select value={sourceFilter} onValueChange={setSourceFilter}>
              <SelectTrigger
                size="sm"
                className="h-7 max-w-28 px-2 text-[11px]"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">{t("allSources")}</SelectItem>
                <SelectItem value="declared">{t("declared")}</SelectItem>
                <SelectItem value="inferred">{t("inferred")}</SelectItem>
              </SelectContent>
            </Select>
            <Select value={turnFilter} onValueChange={setTurnFilter}>
              <SelectTrigger
                size="sm"
                className="h-7 max-w-28 px-2 text-[11px]"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">{t("allTurns")}</SelectItem>
                {turnIds.map((turnRunId, index) => (
                  <SelectItem key={turnRunId} value={turnRunId}>
                    {t("turnNumber", { number: index + 1 })}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select value={timeFilter} onValueChange={changeTimeFilter}>
              <SelectTrigger
                size="sm"
                className="h-7 max-w-28 px-2 text-[11px]"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">{t("allTimes")}</SelectItem>
                <SelectItem value="day">{t("lastDay")}</SelectItem>
                <SelectItem value="week">{t("lastWeek")}</SelectItem>
                <SelectItem value="month">{t("lastMonth")}</SelectItem>
              </SelectContent>
            </Select>
            <Select value={sort} onValueChange={setSort}>
              <SelectTrigger
                size="sm"
                className="h-7 max-w-28 px-2 text-[11px]"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="newest">{t("newest")}</SelectItem>
                <SelectItem value="oldest">{t("oldest")}</SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div className="flex items-center gap-1 border-b px-2 py-1.5">
            <Checkbox
              checked={allFilteredSelected}
              onCheckedChange={(value) => toggleAll(value === true)}
              aria-label={t("selectAll")}
            />
            <span className="me-auto text-[10px] text-muted-foreground">
              {t("selectedCount", { count: selectedItems.length })}
            </span>
            <TooltipProvider>
              {batchActions.map((action) => {
                const Icon = action.icon
                return (
                  <Tooltip key={action.id}>
                    <TooltipTrigger asChild>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-xs"
                        disabled={!action.enabled}
                        aria-label={action.label}
                        onClick={() => void action.action()}
                      >
                        <Icon className="size-3.5" />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>{action.label}</TooltipContent>
                  </Tooltip>
                )
              })}
            </TooltipProvider>
          </div>

          {loading && !loaded ? (
            <div className="flex items-center justify-center gap-2 px-4 py-8 text-xs text-muted-foreground">
              <Loader2 className="size-3.5 animate-spin" />
              {t("loadingHistory")}
            </div>
          ) : error && !loaded ? (
            <div className="flex flex-col items-center gap-2 px-4 py-8 text-center text-xs text-destructive">
              <span>{error}</span>
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={() => void loadPage(0, false)}
              >
                <RefreshCw className="me-1 size-3.5" />
                {t("retry")}
              </Button>
            </div>
          ) : filtered.length === 0 ? (
            <div className="px-4 py-8 text-center text-xs text-muted-foreground">
              {visible.length === 0 ? t("empty") : t("noFilterResults")}
            </div>
          ) : (
            <ul className="max-h-[25rem] space-y-1 overflow-y-auto p-2">
              {filtered.map((item) => {
                const Icon = item.kind === "directory" ? FolderIcon : FileIcon
                const historyGroup = groupById.get(item.id)
                const versions = historyGroup?.versions ?? []
                return (
                  <li
                    key={historyGroup?.path_key ?? item.id}
                    title={item.description ?? item.title}
                    className="rounded-md border border-border/70 px-2 py-1.5"
                  >
                    <div className="flex items-center gap-2">
                      <Checkbox
                        checked={selected.has(item.id)}
                        onCheckedChange={(checked) =>
                          setSelected((previous) => {
                            const next = new Set(previous)
                            if (checked === true) next.add(item.id)
                            else next.delete(item.id)
                            return next
                          })
                        }
                        aria-label={t("selectFile", { name: item.file_name })}
                      />
                      <Icon className="size-4 shrink-0 text-muted-foreground" />
                      <div className="min-w-0 flex-1">
                        <div className="flex min-w-0 items-center gap-1">
                          <span className="truncate text-xs font-medium">
                            {item.file_name || item.title}
                          </span>
                          {item.source === "inferred" && (
                            <Badge
                              variant="outline"
                              className="h-4 px-1 text-[9px]"
                            >
                              {t("inferred")}
                            </Badge>
                          )}
                        </div>
                        <div
                          data-testid={`deliverable-metadata-${item.id}`}
                          className="flex min-w-0 items-center gap-1 whitespace-nowrap text-[10px] text-muted-foreground"
                        >
                          <span className="shrink-0">
                            {(item.extension ?? item.kind).toUpperCase()}
                          </span>
                          <span aria-hidden="true" className="shrink-0">
                            |
                          </span>
                          <time
                            dateTime={item.produced_at}
                            className="shrink-0"
                          >
                            {formatCompactTimestamp(item.produced_at)}
                          </time>
                          <span aria-hidden="true" className="shrink-0">
                            |
                          </span>
                          <span className="shrink-0">
                            {formatBytes(item.size_bytes)}
                          </span>
                        </div>
                      </div>
                      <DeliverableFileActions
                        conversationId={conversationId}
                        item={item}
                      />
                    </div>
                    {versions.length > 1 ? (
                      <details className="ms-8 mt-1 text-[10px] text-muted-foreground">
                        <summary
                          className="cursor-pointer select-none"
                          aria-label={t("toggleVersions", {
                            count: versions.length,
                          })}
                        >
                          {t("versions", { count: versions.length })}
                        </summary>
                        <ul className="mt-1 space-y-0.5 border-s ps-2">
                          {versions.map((version) => (
                            <li
                              key={`${version.turn_run_id}:${version.source}:${version.produced_at}`}
                            >
                              {new Date(version.produced_at).toLocaleString()} ·{" "}
                              {version.source === "declared"
                                ? t("declared")
                                : t("inferred")}
                            </li>
                          ))}
                        </ul>
                      </details>
                    ) : null}
                  </li>
                )
              })}
            </ul>
          )}
          {loaded && nextOffset !== null ? (
            <div className="border-t p-2 text-center">
              <Button
                type="button"
                size="sm"
                variant="ghost"
                disabled={loading}
                onClick={() => void loadPage(nextOffset, true)}
              >
                {loading ? (
                  <Loader2 className="me-1 size-3.5 animate-spin" />
                ) : null}
                {t("loadMore")}
              </Button>
            </div>
          ) : null}
          {capabilities?.hostActionNotice && (
            <div className="border-t px-3 py-1.5 text-[9px] text-muted-foreground">
              {t("hostActionNotice")}
            </div>
          )}
        </div>
      </div>
    )
  }
)
