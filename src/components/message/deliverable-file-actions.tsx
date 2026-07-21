"use client"

import { useState } from "react"
import {
  ClipboardCopy,
  Copy,
  Download,
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
  revealDeliverable,
} from "@/lib/api"
import { toAbsoluteFilePath } from "@/lib/file-path-display"
import type { ConversationDeliverable } from "@/lib/types"
import { copyTextToClipboard } from "@/lib/utils"

type Action = "download" | "copyFile" | "copyPath" | "reveal"

export function DeliverableFileActions({
  conversationId,
  item,
}: {
  conversationId: number
  item: ConversationDeliverable
}) {
  const t = useTranslations("Folder.chat.conversationDeliverables")
  const capabilities = useDeliverableCapabilities()
  const [pending, setPending] = useState<Action | null>(null)

  const run = async (action: Action, operation: () => Promise<unknown>) => {
    if (pending !== null || (!item.is_valid && action !== "copyPath")) return
    setPending(action)
    try {
      await operation()
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
      id: "download" as const,
      label: t("download"),
      icon: Download,
      visible: true,
      operation: () =>
        downloadDeliverables({
          conversationId,
          deliverableIds: [item.id],
          archive: item.kind === "directory",
          suggestedName:
            item.kind === "directory"
              ? `${item.file_name}.zip`
              : item.file_name,
        }),
    },
    {
      id: "copyFile" as const,
      label: t("copyFileHost"),
      icon: ClipboardCopy,
      visible: capabilities?.copyFiles === true,
      operation: () => copyDeliverableFiles(conversationId, [item.id]),
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
      operation: () => revealDeliverable(conversationId, item.id),
    },
  ]

  return (
    <TooltipProvider>
      <div className="flex shrink-0 items-center gap-0.5">
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
