import { render, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { useConnectionLifecycle } from "@/hooks/use-connection-lifecycle"
import type { AgentType } from "@/lib/types"

const h = vi.hoisted(() => ({
  connect: vi.fn(async () => undefined),
  disconnect: vi.fn(async () => undefined),
  setActiveKey: vi.fn(),
  touchActivity: vi.fn(),
}))

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))

vi.mock("@/contexts/acp-connections-context", () => ({
  useAcpActions: () => ({
    setActiveKey: h.setActiveKey,
    touchActivity: h.touchActivity,
  }),
}))

vi.mock("@/contexts/task-context", () => ({
  useTaskContext: () => ({
    addTask: vi.fn(),
    updateTask: vi.fn(),
    removeTask: vi.fn(),
  }),
}))

vi.mock("@/hooks/use-connection", () => ({
  useConnection: () => ({
    connectionId: null,
    conversationId: null,
    agentType: null,
    isViewer: false,
    status: null,
    promptCapabilities: {
      image: false,
      audio: false,
      embedded_context: false,
    },
    supportsFork: false,
    supportsSteer: false,
    selectorsReady: false,
    hasCachedSelectors: false,
    sessionId: null,
    codegMcpAvailable: false,
    mcpServerCount: 0,
    connectedWorkingDir: null,
    modes: null,
    configOptions: null,
    availableCommands: null,
    pendingPermission: null,
    pendingUserMessage: null,
    steerMessages: [],
    pendingQuestion: null,
    pendingAskQuestion: null,
    claudeApiRetry: null,
    error: null,
    loadError: null,
    configStale: false,
    configStaleKind: null,
    configStaleDismissed: false,
    isDelegationChild: false,
    backgroundOutstanding: 0,
    backgroundSettleSyncingSince: null,
    connect: h.connect,
    disconnect: h.disconnect,
    reapplyConfig: vi.fn(),
    dismissConfigStale: vi.fn(),
    sendPrompt: vi.fn(),
    setMode: vi.fn(),
    setConfigOption: vi.fn(),
    cancel: vi.fn(),
    respondPermission: vi.fn(),
    answerQuestion: vi.fn(),
  }),
}))

interface HarnessProps {
  sessionId?: string
  conversationId?: number
  agentType?: AgentType
}

function Harness({
  sessionId,
  conversationId,
  agentType = "codex",
}: HarnessProps) {
  useConnectionLifecycle({
    contextKey: "codex-tab",
    agentType,
    isActive: true,
    workingDir: "/workspace",
    sessionId,
    conversationId,
  })
  return null
}

describe("useConnectionLifecycle persisted identity changes", () => {
  beforeEach(() => {
    h.connect.mockClear()
    h.disconnect.mockClear()
    h.setActiveKey.mockClear()
    h.touchActivity.mockClear()
  })

  it("connects again when an asynchronously loaded external session id becomes available", async () => {
    const view = render(<Harness conversationId={42} />)
    await waitFor(() => {
      expect(h.connect).toHaveBeenCalledWith(
        "codex",
        "/workspace",
        undefined,
        42
      )
    })

    view.rerender(<Harness conversationId={42} sessionId="codex-session-42" />)
    await waitFor(() => {
      expect(h.connect).toHaveBeenLastCalledWith(
        "codex",
        "/workspace",
        "codex-session-42",
        42
      )
    })
    expect(h.connect).toHaveBeenCalledTimes(2)
  })

  it("does not reconnect for an identical repeat render", async () => {
    const props = {
      conversationId: 42,
      sessionId: "codex-session-42",
    }
    const view = render(<Harness {...props} />)
    await waitFor(() => expect(h.connect).toHaveBeenCalledTimes(1))
    view.rerender(<Harness {...props} />)
    await Promise.resolve()
    expect(h.connect).toHaveBeenCalledTimes(1)
  })
})
