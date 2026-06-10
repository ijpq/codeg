import { Bot, FileText, Folder, GitCommit, Hash, Sparkles } from "lucide-react"

import { AgentIcon } from "@/components/agent-icon"
import {
  STATUS_COLORS,
  type AgentType,
  type ConversationStatus,
} from "@/lib/types"
import { cn } from "@/lib/utils"

import type { ReferenceAttrs } from "../types"

const ICON_CLASS = "size-3.5 shrink-0"

export function ReferenceIcon({ data }: { data: ReferenceAttrs }) {
  const meta = data.meta
  switch (data.refType) {
    case "file":
      return meta?.fileKind === "dir" ? (
        <Folder className={ICON_CLASS} />
      ) : (
        <FileText className={ICON_CLASS} />
      )
    case "agent": {
      const agentType = meta?.agentType ?? (data.id as AgentType)
      return agentType ? (
        <AgentIcon agentType={agentType} className={ICON_CLASS} />
      ) : (
        <Bot className={ICON_CLASS} />
      )
    }
    case "session":
      return meta?.agentType ? (
        <AgentIcon agentType={meta.agentType} className={ICON_CLASS} />
      ) : (
        <Hash className={ICON_CLASS} />
      )
    case "commit":
      return <GitCommit className={ICON_CLASS} />
    case "skill":
      return <Sparkles className={ICON_CLASS} />
    default:
      return null
  }
}

export interface ReferenceBadgeProps {
  data: ReferenceAttrs
  className?: string
}

/**
 * Presentational inline chip for a reference. Shared by the editor node view and
 * (later) message-transcript rendering. Purely visual — no editor coupling.
 */
export function ReferenceBadge({ data, className }: ReferenceBadgeProps) {
  const statusColor =
    data.refType === "session" && data.meta?.status
      ? STATUS_COLORS[data.meta.status as ConversationStatus]
      : undefined

  return (
    <span
      data-reference-badge=""
      data-ref-type={data.refType}
      title={data.uri ?? data.label}
      className={cn(
        "inline-flex max-w-[18rem] items-center gap-1 rounded-md border border-border/60 bg-muted/60 px-1.5 py-px align-baseline text-[0.85em] leading-snug text-foreground",
        className
      )}
    >
      <ReferenceIcon data={data} />
      <span className="truncate">{data.label || data.id}</span>
      {statusColor && (
        <span
          aria-hidden
          className={cn("size-1.5 shrink-0 rounded-full", statusColor)}
        />
      )}
    </span>
  )
}
