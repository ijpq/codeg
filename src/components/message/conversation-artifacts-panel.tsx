"use client"

import { memo } from "react"
import {
  ChevronDownIcon,
  ExternalLink,
  FileDiff,
  FileIcon,
  Files,
} from "lucide-react"
import { toast } from "sonner"
import { useTranslations } from "next-intl"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useWorkspaceActions } from "@/contexts/workspace-context"
import { resolveAvailableArtifactPath } from "@/lib/artifact-file-target"
import type { FileChangeStat } from "@/lib/session-files"
import type { FolderDetail } from "@/lib/types"
import { CollapsedOverlayChip } from "@/components/chat/collapsed-overlay-chip"
import {
  CommitFileAdditions,
  CommitFileDeletions,
} from "@/components/ai-elements/commit"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  fileNameOf,
  isRemovedFileDiff,
  toAbsoluteFilePath,
  toFolderRelativePath,
} from "@/lib/file-path-display"
import { isLocalDesktop, revealItemInDir } from "@/lib/platform"
import { cn } from "@/lib/utils"

interface ConversationArtifactsPanelProps {
  /** Distinct produced/changed file count — drives the collapsed chip and the
   *  header badge. Derived cheaply by the parent (`countSessionArtifactFiles`,
   *  no diff parsing) so the chip stays live without taxing the streaming hot
   *  path. */
  count: number
  /** Whether the panel is expanded. Owned by the parent so the expensive
   *  `files` (full diffs via `extractReplyFileChanges`) is computed lazily —
   *  only while open. */
  expanded: boolean
  onToggle: (next: boolean) => void
  /** The whole conversation's produced files, deduped by path. Only populated
   *  while `expanded` (computed lazily by the parent). */
  files: FileChangeStat[]
  /** This conversation's actual workspace root. Chat mode supplies its hidden
   *  per-conversation folder here, even though it is absent from folder lists. */
  folder?: FolderDetail | null
}

/**
 * Per-conversation "produced files" panel. Lives in the inline-start overlay
 * stack alongside the message navigator (the conversation's de-facto header).
 *
 * Collapsed (default): a `CollapsedOverlayChip` showing the produced-file count
 * on hover. Expanded: a card listing every file the conversation created or
 * changed (deduped across the whole thread), each row opening the file in the
 * workspace tabs (preview), with a side button for its diff and, on desktop, a
 * reveal-in-file-manager button. `memo`'d so it never re-renders while collapsed
 * during streaming (its props are referentially stable then).
 */
export const ConversationArtifactsPanel = memo(
  function ConversationArtifactsPanel({
    count,
    expanded,
    onToggle,
    files,
    folder: sessionFolder,
  }: ConversationArtifactsPanelProps) {
    const t = useTranslations("Folder.chat.conversationArtifacts")
    const { openFilePreview, openSessionFileDiff } = useWorkspaceActions()
    const { activeFolder } = useActiveFolder()
    const folder = sessionFolder === undefined ? activeFolder : sessionFolder

    if (count <= 0) return null

    if (!expanded) {
      // Positioning is owned by the shared overlay-stack container in
      // MessageListView; the chip only declares its icon + summary.
      return (
        <CollapsedOverlayChip
          icon={<Files className="size-3.5" />}
          summary={t("collapsedSummary", { count })}
          onClick={() => onToggle(true)}
        />
      )
    }

    const folderPath = folder?.path

    const openInTab = async (file: FileChangeStat) => {
      const rel = toFolderRelativePath(file.path, folderPath)
      let absolutePath: string
      try {
        absolutePath = await resolveAvailableArtifactPath(file.path, folderPath)
      } catch {
        toast.error(t("fileUnavailable", { filePath: rel }))
        return
      }
      void openFilePreview(absolutePath, {
        folderId: folder?.id,
      })
    }

    const openDiff = (file: FileChangeStat) => {
      if (!file.diff) {
        toast.error(t("fileUnavailable", { filePath: file.path }))
        return
      }
      // Pass the folder explicitly: `openSessionFileDiff` silently returns when
      // it can't resolve a target folder from the (absent) default.
      openSessionFileDiff(file.path, file.diff, `artifact-${file.id}`, {
        folderId: folder?.id,
      })
    }

    const reveal = (file: FileChangeStat) => {
      const absolute = toAbsoluteFilePath(file.path, folderPath)
      if (absolute) void revealItemInDir(absolute)
    }

    return (
      <div className="pointer-events-none flex max-w-[min(22rem,calc(100%-2rem))]">
        <div className="pointer-events-auto w-72 max-w-full rounded-xl border bg-card/60 shadow-lg backdrop-blur transition-colors hover:bg-card/95 supports-[backdrop-filter]:bg-card/50 supports-[backdrop-filter]:hover:bg-card/85">
          <div className="flex items-center justify-between border-b px-3 py-2">
            <div className="flex min-w-0 items-center gap-2">
              <Files className="h-4 w-4 text-muted-foreground" />
              <span className="truncate text-sm font-medium">{t("title")}</span>
              <Badge variant="secondary" className="h-5">
                {count}
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

          {files.length === 0 ? (
            <p className="px-3 py-4 text-center text-xs text-muted-foreground">
              {t("empty")}
            </p>
          ) : (
            <ul className="max-h-96 space-y-1 overflow-y-auto p-2">
              {files.map((file) => {
                const displayPath = toFolderRelativePath(file.path, folderPath)
                const name = fileNameOf(displayPath)
                const dir =
                  displayPath === name
                    ? ""
                    : displayPath.slice(0, displayPath.length - name.length - 1)
                const isRemoved = isRemovedFileDiff(file.diff)

                return (
                  <li
                    key={file.id}
                    className={cn(
                      "flex items-stretch overflow-hidden rounded-md border",
                      isRemoved ? "border-destructive/30" : "border-border"
                    )}
                  >
                    <button
                      type="button"
                      onClick={() => void openInTab(file)}
                      title={displayPath}
                      aria-label={t("openFile", { filePath: displayPath })}
                      className="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left transition-colors hover:bg-accent/40"
                    >
                      <FileIcon
                        className={cn(
                          "h-3.5 w-3.5 shrink-0",
                          isRemoved
                            ? "text-destructive"
                            : "text-muted-foreground"
                        )}
                      />
                      <span className="flex min-w-0 flex-1 flex-col">
                        <span
                          className={cn(
                            "truncate text-xs",
                            isRemoved ? "text-destructive" : "text-foreground"
                          )}
                        >
                          {name}
                        </span>
                        {dir && (
                          <span className="truncate text-[10px] text-muted-foreground">
                            {dir}
                          </span>
                        )}
                      </span>
                      {isRemoved ? (
                        <span className="inline-flex shrink-0 items-center rounded-md border border-destructive/30 bg-destructive/10 px-1.5 py-0.5 font-mono text-[10px] text-destructive">
                          {t("removed")}
                        </span>
                      ) : (
                        <span className="inline-flex shrink-0 items-center gap-1 rounded-md border border-border bg-muted/40 px-1.5 py-0.5 font-mono text-[10px] text-foreground">
                          <CommitFileAdditions
                            count={file.additions}
                            className="text-[10px]"
                          />
                          <CommitFileDeletions
                            count={file.deletions}
                            className="text-[10px]"
                          />
                        </span>
                      )}
                    </button>

                    {file.diff && (
                      <button
                        type="button"
                        aria-label={t("viewDiff")}
                        onClick={() => openDiff(file)}
                        className="flex w-7 shrink-0 items-center justify-center border-s border-border text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground"
                      >
                        <FileDiff className="h-3.5 w-3.5" />
                      </button>
                    )}

                    {isLocalDesktop() && (
                      <button
                        type="button"
                        aria-label={t("revealInFolder")}
                        onClick={() => reveal(file)}
                        className="flex w-7 shrink-0 items-center justify-center border-s border-border text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground"
                      >
                        <ExternalLink className="h-3.5 w-3.5" />
                      </button>
                    )}
                  </li>
                )
              })}
            </ul>
          )}
        </div>
      </div>
    )
  }
)
