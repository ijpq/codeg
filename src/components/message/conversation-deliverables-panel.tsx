"use client"

import { memo, useMemo, useState } from "react"
import {
  Archive,
  ChevronDownIcon,
  ClipboardCopy,
  Download,
  FileIcon,
  FolderIcon,
  FolderSearch,
  PackageCheck,
  Trash2,
  TriangleAlert,
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
  hideDeliverables,
  revealDeliverable,
} from "@/lib/api"
import type {
  ConversationDeliverable,
  ConversationTurnDeliverableSet,
} from "@/lib/types"

interface ConversationDeliverablesPanelProps {
  conversationId: number
  expanded: boolean
  onToggle: (next: boolean) => void
  deliverables: ConversationDeliverable[]
  deliverableRuns?: ConversationTurnDeliverableSet[]
}

function formatBytes(value?: number | null): string {
  if (value == null) return "—"
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  if (value < 1024 * 1024 * 1024) {
    return `${(value / (1024 * 1024)).toFixed(1)} MB`
  }
  return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GB`
}

export const ConversationDeliverablesPanel = memo(
  function ConversationDeliverablesPanel({
    conversationId,
    expanded,
    onToggle,
    deliverables,
    deliverableRuns = [],
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
    const [locallyHidden, setLocallyHidden] = useState<Map<string, string>>(
      () => new Map()
    )
    const visible = useMemo(
      () =>
        deliverables.filter(
          (item) => locallyHidden.get(item.id) !== item.updated_at
        ),
      [deliverables, locallyHidden]
    )
    const extensions = useMemo(
      () =>
        [...new Set(visible.map((item) => item.extension ?? item.kind))].sort(),
      [visible]
    )
    const turnLabels = useMemo(() => {
      const labels = new Map<string, number>()
      deliverableRuns.forEach((run, index) =>
        labels.set(run.turn_run_id, index + 1)
      )
      return labels
    }, [deliverableRuns])
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
          summary={t("collapsedSummary", { count: visible.length })}
          onClick={() => onToggle(true)}
        />
      )
    }

    const selectedItems = visible.filter((item) => selected.has(item.id))
    const validSelected = selectedItems.filter((item) => item.is_valid)
    const invalidSelected = selectedItems.filter((item) => !item.is_valid)
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
    const removeInvalid = async () => {
      const ids = invalidSelected.map((item) => item.id)
      if (ids.length === 0) return
      await runBatch(async () => {
        await hideDeliverables(conversationId, ids)
        setLocallyHidden((previous) => {
          const next = new Map(previous)
          invalidSelected.forEach((item) => next.set(item.id, item.updated_at))
          return next
        })
        setSelected((previous) => {
          const next = new Set(previous)
          ids.forEach((id) => next.delete(id))
          return next
        })
      }, t("removed"))
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
      {
        id: "remove",
        label: t("removeInvalid"),
        icon: Trash2,
        enabled: invalidSelected.length > 0,
        action: removeInvalid,
      },
    ]

    return (
      <div className="pointer-events-none flex w-[29rem] max-w-[calc(100vw-2rem)]">
        <div className="pointer-events-auto w-full overflow-hidden rounded-xl border bg-card/90 shadow-lg backdrop-blur">
          <div className="flex items-center justify-between border-b px-3 py-2">
            <div className="flex min-w-0 items-center gap-2">
              <PackageCheck className="size-4 text-muted-foreground" />
              <span className="truncate text-sm font-medium">{t("title")}</span>
              <Badge variant="secondary" className="h-5">
                {visible.length}
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
                {deliverableRuns.map((run, index) => (
                  <SelectItem key={run.turn_run_id} value={run.turn_run_id}>
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

          {filtered.length === 0 ? (
            <div className="px-4 py-8 text-center text-xs text-muted-foreground">
              {visible.length === 0 ? t("empty") : t("noFilterResults")}
            </div>
          ) : (
            <ul className="max-h-[25rem] space-y-1 overflow-y-auto p-2">
              {filtered.map((item) => {
                const Icon = item.kind === "directory" ? FolderIcon : FileIcon
                return (
                  <li
                    key={item.id}
                    title={item.description ?? item.title}
                    className="flex items-center gap-2 rounded-md border border-border/70 px-2 py-1.5"
                  >
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
                        {!item.is_valid && (
                          <Badge
                            variant="destructive"
                            className="h-4 gap-0.5 px-1 text-[9px]"
                          >
                            <TriangleAlert className="size-2.5" />
                            {t("missing")}
                          </Badge>
                        )}
                        {item.source === "inferred" && (
                          <Badge
                            variant="outline"
                            className="h-4 px-1 text-[9px]"
                          >
                            {t("inferred")}
                          </Badge>
                        )}
                      </div>
                      <div className="truncate text-[10px] text-muted-foreground">
                        {item.title && item.title !== item.file_name
                          ? `${item.title} · `
                          : ""}
                        {formatBytes(item.size_bytes)} · {t("producedAt")}{" "}
                        {new Date(item.produced_at).toLocaleString()} ·{" "}
                        {t("updatedAt")}{" "}
                        {new Date(item.updated_at).toLocaleString()}
                        {item.turn_run_id && turnLabels.has(item.turn_run_id)
                          ? ` · ${t("turnNumber", {
                              number: turnLabels.get(item.turn_run_id) ?? 1,
                            })}`
                          : ""}
                      </div>
                    </div>
                    <DeliverableFileActions
                      conversationId={conversationId}
                      item={item}
                    />
                  </li>
                )
              })}
            </ul>
          )}
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
