"use client"

import {
  FileArchive,
  FileIcon,
  FolderIcon,
  PackageCheck,
  TriangleAlert,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { Badge } from "@/components/ui/badge"
import { DeliverableFileActions } from "./deliverable-file-actions"
import type { ConversationDeliverable } from "@/lib/types"

function formatBytes(value?: number | null): string {
  if (value == null) return "—"
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  if (value < 1024 * 1024 * 1024) {
    return `${(value / (1024 * 1024)).toFixed(1)} MB`
  }
  return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GB`
}

function iconFor(item: ConversationDeliverable) {
  if (item.kind === "directory") return FolderIcon
  if (["zip", "rar", "7z"].includes(item.extension ?? "")) return FileArchive
  return FileIcon
}

export function ReplyDeliverables({
  conversationId,
  deliverables,
}: {
  conversationId: number
  deliverables: ConversationDeliverable[]
}) {
  const t = useTranslations("Folder.chat.conversationDeliverables")
  if (deliverables.length === 0) return null
  return (
    <section
      className="ms-0 mt-2 max-w-2xl rounded-lg border border-border/70 bg-muted/20 p-2"
      aria-label={t("turnTitle")}
    >
      <div className="mb-1.5 flex items-center gap-1.5 px-1 text-xs font-medium text-muted-foreground">
        <PackageCheck className="size-3.5" />
        <span>{t("turnTitle")}</span>
        <Badge variant="secondary" className="h-4 px-1 text-[9px]">
          {deliverables.length}
        </Badge>
      </div>
      <ul className="space-y-1">
        {deliverables.map((item) => {
          const Icon = iconFor(item)
          return (
            <li
              key={item.id}
              className="flex items-center gap-2 rounded-md bg-background/70 px-2 py-1.5"
            >
              <Icon className="size-4 shrink-0 text-muted-foreground" />
              <div className="min-w-0 flex-1">
                <div className="flex min-w-0 items-center gap-1.5">
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
                  <Badge variant="outline" className="h-4 px-1 text-[9px]">
                    {t(item.role === "supporting" ? "supporting" : "primary")}
                  </Badge>
                  {item.source === "inferred" && (
                    <Badge variant="outline" className="h-4 px-1 text-[9px]">
                      {t("inferred")}
                    </Badge>
                  )}
                </div>
                <div className="truncate text-[10px] text-muted-foreground">
                  {item.title && item.title !== item.file_name
                    ? `${item.title} · `
                    : ""}
                  {formatBytes(item.size_bytes)}
                  {item.description ? ` · ${item.description}` : ""}
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
    </section>
  )
}
