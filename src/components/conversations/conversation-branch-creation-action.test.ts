import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type {
  CreateConversationBranchRequest,
  CreateConversationBranchResult,
} from "@/lib/api"

const h = vi.hoisted(() => ({
  createConversationBranch:
    vi.fn<
      (
        request: CreateConversationBranchRequest
      ) => Promise<CreateConversationBranchResult>
    >(),
  getCachedSelectors: vi.fn(() => ({
    modes: { current_mode_id: "plan" },
    configOptions: [
      { id: "model", kind: { current_value: "gpt-5.5" } },
      { id: "effort", kind: { current_value: "high" } },
    ],
  })),
}))

vi.mock("@/lib/api", () => ({
  createConversationBranch: h.createConversationBranch,
}))
vi.mock("@/contexts/acp-connections-context", () => ({
  getCachedSelectors: h.getCachedSelectors,
}))

import {
  getDatedBranchTitle,
  requestConversationBranchCreation,
} from "./conversation-branch-creation-action"

const CREATED: CreateConversationBranchResult = {
  branchConversationId: 42,
  sourceConversationId: 7,
  folderId: 3,
  connectionId: "connection-42",
  branchSessionId: "session-42",
  sessionReady: true,
  promptReady: true,
  lifecycleState: "ready",
  forkMode: "native",
  inheritanceMode: "native_fork",
  inheritedMessageCount: 20,
  inheritanceTruncated: false,
}
const queuedEvents: CustomEvent[] = []

function acceptQueue(event: Event) {
  queuedEvents.push(event as CustomEvent)
  event.preventDefault()
}

describe("requestConversationBranchCreation", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    queuedEvents.length = 0
    h.createConversationBranch.mockResolvedValue(CREATED)
  })

  afterEach(() => {
    window.removeEventListener(
      "codeg:queue-conversation-branch-creation",
      acceptQueue
    )
  })

  it("hands the operation to the mounted source tab queue", async () => {
    window.addEventListener(
      "codeg:queue-conversation-branch-creation",
      acceptQueue
    )

    const result = await requestConversationBranchCreation({
      conversationId: 7,
      agentType: "codex",
      requestId: "operation-7",
    })

    expect(result).toEqual({ kind: "queued", requestId: "operation-7" })
    expect(queuedEvents[0]?.detail).toEqual({
      conversationId: 7,
      requestId: "operation-7",
      operationId: "operation-7",
      modeId: "plan",
    })
    expect(h.createConversationBranch).not.toHaveBeenCalled()
  })

  it("uses the same backend request when no source tab is mounted", async () => {
    const result = await requestConversationBranchCreation({
      conversationId: 7,
      agentType: "codex",
      requestId: "operation-7",
    })

    expect(result).toEqual({
      kind: "created",
      requestId: "operation-7",
      result: CREATED,
    })
    expect(h.createConversationBranch).toHaveBeenCalledWith({
      requestId: "operation-7",
      operationId: "operation-7",
      sourceConversationId: 7,
      deferIfSourceBusy: false,
      preferredModeId: "plan",
      preferredConfigValues: {
        model: "gpt-5.5",
        effort: "high",
      },
    })
  })
})

describe("getDatedBranchTitle", () => {
  it("uses the dotted month/day form", () => {
    expect(getDatedBranchTitle("Agent 调试", new Date(2026, 7, 25))).toBe(
      "Agent 调试 · 分支 8.25"
    )
  })
})
