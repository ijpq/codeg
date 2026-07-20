"use client"

import { memo, useEffect, useMemo } from "react"
import {
  ChevronDownIcon,
  Download,
  ExternalLink,
  FileIcon,
  FolderIcon,
  PackageCheck,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { useWorkspaceActions } from "@/contexts/workspace-context"
import { downloadWorkspaceDir, downloadWorkspaceFile } from "@/lib/api"
import { resolveAvailableArtifactPath } from "@/lib/artifact-file-target"
import {
  hasSyncedProducedFile,
  markProducedFileSynced,
  useAutoDownloadProduced,
} from "@/lib/produced-file-sync-prefs"
import type { ConversationDeliverable, FolderDetail } from "@/lib/types"
import { CollapsedOverlayChip } from "@/components/chat/collapsed-overlay-chip"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { fileNameOf, toAbsoluteFilePath } from "@/lib/file-path-display"
import { isLocalDesktop, revealItemInDir } from "@/lib/platform"

interface ConversationDeliverablesPanelProps {
  expanded: boolean
  onToggle: (next: boolean) => void
  deliverables: ConversationDeliverable[]
  folder?: FolderDetail | null
}

/**
 * Conversation-level final outputs. Every item came from an explicit agent
 * declaration and was canonicalized + existence-checked by the backend. This
 * component intentionally has no transcript or filesystem-change fallback: an
 * undeclared edit is not a deliverable.
 */
export const ConversationDeliverablesPanel = memo(
  function ConversationDeliverablesPanel({
    expanded,
    onToggle,
    deliverables,
    folder,
  }: ConversationDeliverablesPanelProps) {
    const t = useTranslations("Folder.chat.conversationDeliverables")
    const { openFilePreview } = useWorkspaceActions()
    const autoDownloadEnabled = useAutoDownloadProduced()
    const ordered = useMemo(
      () =>
        [...deliverables].sort((left, right) => {
          if (left.role === right.role) return left.position - right.position
          return left.role === "primary" ? -1 : 1
        }),
      [deliverables]
    )

    useEffect(() => {
      if (!autoDownloadEnabled || isLocalDesktop()) return
      for (const item of deliverables) {
        const key = `deliverable:${item.id}:${item.updated_at}`
        if (hasSyncedProducedFile(key)) continue
        markProducedFileSynced(key)
        const download =
          item.kind === "directory"
            ? downloadWorkspaceDir
            : downloadWorkspaceFile
        void download(item.root_path, item.path, fileNameOf(item.path)).catch(
          (error) =>
            console.error(
              "[ConversationDeliverables] auto-download failed:",
              error
            )
        )
      }
    }, [autoDownloadEnabled, deliverables])

    if (deliverables.length === 0) return null

    if (!expanded) {
      return (
        <CollapsedOverlayChip
          icon={<PackageCheck className="size-3.5" />}
          summary={t("collapsedSummary", { count: deliverables.length })}
          onClick={() => onToggle(true)}
        />
      )
    }

    const open = async (item: ConversationDeliverable) => {
      if (item.kind === "directory") {
        const absolute = toAbsoluteFilePath(item.path, item.root_path)
        if (absolute && isLocalDesktop()) void revealItemInDir(absolute)
        return
      }
      let absolutePath: string
      try {
        absolutePath = await resolveAvailableArtifactPath(
          item.path,
          item.root_path
        )
      } catch {
        toast.error(t("fileUnavailable", { filePath: item.path }))
        return
      }
      void openFilePreview(absolutePath, { folderId: folder?.id })
    }

    const reveal = (item: ConversationDeliverable) => {
      const absolute = toAbsoluteFilePath(item.path, item.root_path)
      if (absolute) void revealItemInDir(absolute)
    }

    const download = (item: ConversationDeliverable) => {
      const downloadItem =
        item.kind === "directory" ? downloadWorkspaceDir : downloadWorkspaceFile
      void downloadItem(item.root_path, item.path, fileNameOf(item.path)).catch(
        (error) =>
          console.error("[ConversationDeliverables] download failed:", error)
      )
    }

    return (
      <div className="pointer-events-none flex max-w-[min(24rem,calc(100%-2rem))]">
        <div className="pointer-events-auto w-80 max-w-full overflow-hidden rounded-xl border bg-card/60 shadow-lg backdrop-blur transition-colors hover:bg-card/95 supports-[backdrop-filter]:bg-card/50 supports-[backdrop-filter]:hover:bg-card/85">
          <div className="flex items-center justify-between border-b px-3 py-2">
            <div className="flex min-w-0 items-center gap-2">
              <PackageCheck className="h-4 w-4 text-muted-foreground" />
              <span className="truncate text-sm font-medium">{t("title")}</span>
              <Badge variant="secondary" className="h-5">
                {deliverables.length}
              </Badge>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              aria-label={t("collapse")}
              onClick={() => onToggle(false)}
            >
              <ChevronDownIcon className="h-4 w-4" />
            </Button>
          </div>

          <ul className="max-h-96 space-y-1 overflow-y-auto p-2">
            {ordered.map((item) => {
              const ItemIcon = item.kind === "directory" ? FolderIcon : FileIcon
              const canOpen = item.kind === "file" || isLocalDesktop()
              return (
                <li
                  key={item.id}
                  className="flex items-stretch overflow-hidden rounded-md border border-border"
                >
                  <button
                    type="button"
                    disabled={!canOpen}
                    onClick={() => void open(item)}
                    title={item.path}
                    aria-label={t("openFile", { filePath: item.path })}
                    className="flex min-w-0 flex-1 items-start gap-2 px-2.5 py-2 text-left transition-colors enabled:hover:bg-accent/40 disabled:cursor-default"
                  >
                    <ItemIcon className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
                    <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                      <span className="flex min-w-0 items-center gap-1.5">
                        <span className="truncate text-xs font-medium text-foreground">
                          {item.title}
                        </span>
                        <span className="shrink-0 rounded border bg-muted/40 px-1 py-0.5 text-[9px] text-muted-foreground">
                          {t(
                            item.role === "supporting"
                              ? "supporting"
                              : "primary"
                          )}
                        </span>
                      </span>
                      <span className="truncate text-[10px] text-muted-foreground">
                        {item.path}
                      </span>
                      {item.description && (
                        <span className="line-clamp-2 text-[10px] leading-4 text-muted-foreground">
                          {item.description}
                        </span>
                      )}
                    </span>
                  </button>

                  {isLocalDesktop() ? (
                    <button
                      type="button"
                      aria-label={t("revealInFolder")}
                      onClick={() => reveal(item)}
                      className="flex w-8 shrink-0 items-center justify-center border-s border-border text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground"
                    >
                      <ExternalLink className="h-3.5 w-3.5" />
                    </button>
                  ) : (
                    <button
                      type="button"
                      aria-label={t("downloadToPc")}
                      onClick={() => download(item)}
                      className="flex w-8 shrink-0 items-center justify-center border-s border-border text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground"
                    >
                      <Download className="h-3.5 w-3.5" />
                    </button>
                  )}
                </li>
              )
            })}
          </ul>
        </div>
      </div>
    )
  }
)
