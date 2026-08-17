import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { useConnectionLifecycle } from "@/hooks/use-connection-lifecycle"
import type { AgentType } from "@/lib/types"

const h = vi.hoisted(() => ({
  connect: vi.fn(async () => undefined),
  disconnect: vi.fn(async () => undefined),
  setActiveKey: vi.fn(),
  touchActivity: vi.fn(),
  cancel: vi.fn(),
  refreshSnapshot: vi.fn(),
  reconnect: vi.fn(),
  status: null as "prompting" | "connected" | null,
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
    status: h.status,
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
    cancel: h.cancel,
    refreshSnapshot: h.refreshSnapshot,
    reconnect: h.reconnect,
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
  const lifecycle = useConnectionLifecycle({
    contextKey: "codex-tab",
    agentType,
    isActive: true,
    workingDir: "/workspace",
    sessionId,
    conversationId,
  })
  return (
    <button onClick={lifecycle.handleCancel}>
      {lifecycle.isCancelling ? "stopping" : "stop"}
    </button>
  )
}

describe("useConnectionLifecycle persisted identity changes", () => {
  beforeEach(() => {
    h.connect.mockClear()
    h.disconnect.mockClear()
    h.setActiveKey.mockClear()
    h.touchActivity.mockClear()
    h.cancel.mockReset()
    h.cancel.mockResolvedValue(null)
    h.refreshSnapshot.mockReset()
    h.refreshSnapshot.mockResolvedValue("connected")
    h.reconnect.mockReset()
    h.reconnect.mockResolvedValue(true)
    h.status = null
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

  it("sends only one cancel while a fast double-click is in flight", async () => {
    h.status = "prompting"
    let resolveCancel: (value: {
      outcome: "cancel_requested"
      cancelRequestId: string
    }) => void = () => {}
    h.cancel.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveCancel = resolve
        })
    )
    const view = render(<Harness conversationId={42} sessionId="session-42" />)

    fireEvent.click(screen.getByRole("button"))
    fireEvent.click(screen.getByRole("button"))
    expect(h.cancel).toHaveBeenCalledTimes(1)
    expect(screen.getByText("stopping")).toBeInTheDocument()

    await act(async () => {
      resolveCancel({
        outcome: "cancel_requested",
        cancelRequestId: "request-1",
      })
    })
    expect(screen.getByText("stopping")).toBeInTheDocument()

    h.status = "connected"
    view.rerender(<Harness conversationId={42} sessionId="session-42" />)
    await waitFor(() => expect(screen.getByText("stop")).toBeInTheDocument())
  })

  it("queries authoritative state and reconnects when the terminal event is lost", async () => {
    vi.useFakeTimers()
    try {
      h.status = "prompting"
      h.refreshSnapshot.mockResolvedValue("prompting")
      h.cancel.mockResolvedValue({
        outcome: "cancel_requested",
        cancelRequestId: "request-timeout",
        deadlineAt: new Date(Date.now() + 1_000).toISOString(),
      })
      render(<Harness conversationId={42} sessionId="session-42" />)

      fireEvent.click(screen.getByRole("button"))
      await act(async () => {
        await Promise.resolve()
        await vi.advanceTimersByTimeAsync(13_001)
      })

      expect(h.refreshSnapshot).toHaveBeenCalledTimes(1)
      expect(h.reconnect).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })
})
