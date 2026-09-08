"use client"

import { createConversationBranch } from "@/lib/api"
import type { CreateConversationBranchResult } from "@/lib/api"
import { getCachedSelectors } from "@/contexts/acp-connections-context"
import { queueConversationBranchCreation } from "@/hooks/use-message-queue"
import type { AgentType } from "@/lib/types"

export type ConversationBranchCreationActionResult =
  | { kind: "queued"; requestId: string }
  | {
      kind: "created"
      requestId: string
      result: CreateConversationBranchResult
    }

export function getDatedBranchTitle(
  sourceTitle: string,
  date = new Date()
): string {
  return `${sourceTitle} · 分支 ${date.getMonth() + 1}.${date.getDate()}`
}

/**
 * Start the same latest-tail branch operation used by every conversation-level
 * entry point. A mounted source tab gets first refusal so an active turn can
 * keep the request in its durable FIFO. If no runtime owns the conversation,
 * the backend creates the branch directly from the authoritative persisted
 * state.
 *
 * The caller owns `requestId`: it must retain the id after a transient failure
 * and discard it only after the operation was durably queued or created.
 */
export async function requestConversationBranchCreation({
  conversationId,
  agentType,
  requestId,
}: {
  conversationId: number
  agentType: AgentType
  requestId: string
}): Promise<ConversationBranchCreationActionResult> {
  const selectors = getCachedSelectors(agentType)
  const modeId = selectors?.modes?.current_mode_id ?? null
  const preferredConfigValues = Object.fromEntries(
    (selectors?.configOptions ?? []).map((option) => [
      option.id,
      String(option.kind.current_value),
    ])
  )

  if (
    queueConversationBranchCreation({
      conversationId,
      requestId,
      operationId: requestId,
      modeId,
    })
  ) {
    return { kind: "queued", requestId }
  }

  return {
    kind: "created",
    requestId,
    result: await createConversationBranch({
      requestId,
      operationId: requestId,
      sourceConversationId: conversationId,
      deferIfSourceBusy: false,
      preferredModeId: modeId,
      preferredConfigValues,
    }),
  }
}
