import { getTransport } from "@/lib/transport"
import type { AgentType } from "@/lib/types"

export const BUSINESS_AGENTS = ["codex", "claude_code", "pi"] as const

export const supportsBusinessHandoff = (agent: AgentType) =>
  BUSINESS_AGENTS.some((supported) => supported === agent)

export interface BusinessTaskState {
  objective: string
  requirements: string[]
  completed: string[]
  pending: string[]
  latestFiles: string[]
  decisions: string[]
}
export interface BusinessAgentSession {
  id: number
  agentType: AgentType
  externalId: string
  modeId: string | null
  configValues: Record<string, string>
  seenSequence: number
}
export interface BusinessConversationState {
  conversationId: number
  revision: number
  activeAgent: AgentType
  pendingRequestId: string | null
  pendingAgent: AgentType | null
  switchStatus: string | null
  switchError: string | null
  switchPhase?: string | null
  taskState: BusinessTaskState
  sessions: BusinessAgentSession[]
  lastSequence: number
}
export interface SharedBusinessMemory {
  scope: "global" | "project" | "task"
  scopeId: number
  name: string
  content: string
  source: string
  revision: number
  updatedAt: string
}
export const getBusinessState = (conversationId: number) =>
  getTransport().call<BusinessConversationState | null>(
    "get_business_conversation_state",
    { conversationId }
  )
export const switchBusinessAgent = (request: {
  conversationId: number
  requestId: string
  targetAgent: AgentType
  expectedRevision: number | null
  preferredModeId?: string | null
  preferredConfigValues?: Record<string, string>
}) =>
  getTransport().call<BusinessConversationState>(
    "switch_business_agent",
    { request },
    { timeoutMs: 120_000 }
  )
export const updateBusinessTaskState = (
  conversationId: number,
  revision: number,
  task: BusinessTaskState
) =>
  getTransport().call<void>("update_business_task_state", {
    conversationId,
    revision,
    task,
  })
export const getBusinessMemory = (conversationId: number) =>
  getTransport().call<SharedBusinessMemory[]>("get_business_shared_memory", {
    conversationId,
  })
export const saveBusinessMemory = (
  conversationId: number,
  item: SharedBusinessMemory
) =>
  getTransport().call<void>("save_business_shared_memory", {
    conversationId,
    item,
  })
