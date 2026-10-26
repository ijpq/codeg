"use client"

import { useState } from "react"
import {
  ClipboardCopy,
  Copy,
  Download,
  ExternalLink,
  Eye,
  FolderSearch,
  Loader2,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { Button } from "@/components/ui/button"
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
  openDeliverable,
  getOfficeDeliveryPreview,
  revealDeliverable,
} from "@/lib/api"
import { useOptionalWorkspaceActions } from "@/contexts/workspace-context"
import { isOfficePreviewable } from "@/lib/language-detect"
import { toAbsoluteFilePath } from "@/lib/file-path-display"
import type { ConversationDeliverable } from "@/lib/types"
import { cn, copyTextToClipboard } from "@/lib/utils"

type Action =
  | "preview"
  | "download"
  | "open"
  | "copyFile"
  | "copyPath"
  | "reveal"

export function DeliverableFileActions({
  conversationId,
  item,
  className,
}: {
  conversationId: number
  item: ConversationDeliverable
  className?: string
}) {
  const t = useTranslations("Folder.chat.conversationDeliverables")
  const officeT = useTranslations("Folder.fileWorkspacePanel")
  const capabilities = useDeliverableCapabilities()
  const workspace = useOptionalWorkspaceActions()
  // Inherited cards retain the source conversation's verified database ids.
  const ownerConversationId = item.conversation_id ?? conversationId
  const [pending, setPending] = useState<Action | null>(null)

  const run = async (action: Action, operation: () => Promise<unknown>) => {
    if (pending !== null || (!item.is_valid && action !== "copyPath")) return
    setPending(action)
    try {
      await operation()
      if (action === "open") toast.success(t("opened"))
      if (action === "copyFile") toast.success(t("fileCopied"))
      if (action === "copyPath") toast.success(t("pathCopied"))
    } catch (error) {
      console.error(`[Deliverables] ${action} failed`, error)
      toast.error(t("operationFailed"))
    } finally {
      setPending(null)
    }
  }

  const path =
    toAbsoluteFilePath(item.path, item.root_path) ??
    `${item.root_path}/${item.path}`
  const actions = [
    {
      id: "preview" as const,
      label: t("previewInCodeg"),
      icon: Eye,
      visible: Boolean(
        workspace && item.kind === "file" && isOfficePreviewable(path)
      ),
      operation: async () => {
        if (!workspace) return
        const review = item.turn_run_id
          ? await getOfficeDeliveryPreview(
              ownerConversationId,
              item.turn_run_id
            )
          : null
        if (review?.status === "ready" && review.deliverable_id === item.id) {
          workspace.openOfficeDeliveryPreview(review)
        } else {
          if (review?.status === "failed")
            toast.error(officeT("officeReviewFailed", { name: item.file_name }))
          // A manual current-file view stays clearly labelled as such. We
          // never reconstruct an uncaptured historical version from this path.
          await workspace.openFilePreview(path, { reload: true })
        }
      },
    },
    {
      id: "download" as const,
      label: t("download"),
      icon: Download,
      visible: true,
      operation: () =>
        downloadDeliverables({
          conversationId: ownerConversationId,
          deliverableIds: [item.id],
          archive: item.kind === "directory",
          suggestedName:
            item.kind === "directory"
              ? `${item.file_name}.zip`
              : item.file_name,
        }),
    },
    {
      id: "open" as const,
      label: t("openWithDefaultAppHost"),
      icon: ExternalLink,
      visible: capabilities?.openWithDefaultApp === true,
      operation: () => openDeliverable(ownerConversationId, item.id),
    },
    {
      id: "copyFile" as const,
      label: t("copyFileHost"),
      icon: ClipboardCopy,
      visible: capabilities?.copyFiles === true,
      operation: () => copyDeliverableFiles(ownerConversationId, [item.id]),
    },
    {
      id: "copyPath" as const,
      label: t("copyPath"),
      icon: Copy,
      visible: true,
      operation: async () => {
        const copied = await copyTextToClipboard(path)
        if (!copied) throw new Error("clipboard unavailable")
      },
    },
    {
      id: "reveal" as const,
      label: t("revealOnHost"),
      icon: FolderSearch,
      visible: capabilities?.revealInFolder === true,
      operation: () => revealDeliverable(ownerConversationId, item.id),
    },
  ]

  return (
    <TooltipProvider>
      <div className={cn("flex shrink-0 items-center gap-0.5", className)}>
        {actions.map((action) => {
          if (!action.visible) return null
          const Icon = pending === action.id ? Loader2 : action.icon
          return (
            <Tooltip key={action.id}>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  disabled={
                    pending !== null ||
                    (!item.is_valid && action.id !== "copyPath")
                  }
                  aria-label={action.label}
                  onClick={() => void run(action.id, action.operation)}
                >
                  <Icon
                    className={
                      pending === action.id
                        ? "size-3.5 animate-spin"
                        : "size-3.5"
                    }
                  />
                </Button>
              </TooltipTrigger>
              <TooltipContent>{action.label}</TooltipContent>
            </Tooltip>
          )
        })}
      </div>
    </TooltipProvider>
  )
}
