import { act, renderHook, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { useBusinessConversation } from "./use-business-conversation"
import type { BusinessConversationState } from "@/lib/business-conversation"
const api = vi.hoisted(() => ({
  get: vi.fn(),
  switch: vi.fn(),
  subscribe: vi.fn(),
}))
vi.mock("@/lib/business-conversation", () => ({
  getBusinessState: api.get,
  switchBusinessAgent: api.switch,
}))
vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ subscribe: api.subscribe }),
}))
const state = (
  cid = 1,
  revision = 0,
  agent: "codex" | "claude_code" | "pi" = "codex"
): BusinessConversationState => ({
  conversationId: cid,
  revision,
  activeAgent: agent,
  pendingRequestId: null,
  pendingAgent: null,
  switchStatus: null,
  switchError: null,
  taskState: {
    objective: "conference",
    requirements: [],
    completed: [],
    pending: [],
    latestFiles: [],
    decisions: [],
  },
  sessions: [],
  lastSequence: 0,
})
beforeEach(() => {
  localStorage.clear()
  api.get.mockReset()
  api.switch.mockReset()
  api.subscribe.mockResolvedValue(() => {})
  api.get.mockResolvedValue(state())
})
afterEach(() => {
  vi.clearAllMocks()
})
describe("business agent handoff client", () => {
  it("settles from persisted success even if the HTTP acknowledgement is lost", async () => {
    let resolve!: (value: BusinessConversationState) => void
    api.switch.mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done
        })
    )
    const { result } = renderHook(() => useBusinessConversation(1))
    await waitFor(() => expect(result.current.state).not.toBeNull())
    let pending!: Promise<void>
    act(() => {
      pending = result.current.switchAgent("pi")
    })
    await waitFor(() => expect(api.switch).toHaveBeenCalledTimes(1))
    const done = {
      ...state(1, 5, "pi"),
      switchStatus: "succeeded",
      pendingRequestId: api.switch.mock.calls[0][0].requestId,
    }
    api.get.mockResolvedValue(done)
    await act(async () => {
      await result.current.refresh()
    })
    expect(result.current.state?.activeAgent).toBe("pi")
    expect(result.current.pending).toBe(false)
    await act(async () => {
      resolve(done)
      await pending
    })
    expect(api.switch).toHaveBeenCalledTimes(1)
  })
  it("accepts a completed snapshot while a newer refresh is still slow", async () => {
    const { result } = renderHook(() => useBusinessConversation(1))
    await waitFor(() => expect(result.current.state).not.toBeNull())
    let resolveFirst!: (value: BusinessConversationState) => void
    let resolveSecond!: (value: BusinessConversationState) => void
    api.get
      .mockImplementationOnce(
        () =>
          new Promise((done) => {
            resolveFirst = done
          })
      )
      .mockImplementationOnce(
        () =>
          new Promise((done) => {
            resolveSecond = done
          })
      )
    let first!: Promise<void>
    let second!: Promise<void>
    act(() => {
      first = result.current.refresh()
      second = result.current.refresh()
    })
    await act(async () => {
      resolveFirst({ ...state(1, 4, "pi"), switchStatus: "succeeded" })
      await first
    })
    expect(result.current.state?.activeAgent).toBe("pi")
    await act(async () => {
      resolveSecond({ ...state(1, 2), switchStatus: "preparing" })
      await second
    })
    expect(result.current.state?.activeAgent).toBe("pi")
  })
  it("restores Pi settings independently and keeps its native session after refresh", async () => {
    localStorage.setItem(
      "codeg:selector-prefs",
      JSON.stringify({
        codex: { modeId: "plan", configValues: { model: "codex-model" } },
        pi: {
          modeId: "high",
          configValues: {
            model: "pi-provider/pi-model",
            thought_level: "high",
          },
        },
      })
    )
    const pi = {
      ...state(1, 3, "pi"),
      sessions: [
        {
          id: 1,
          agentType: "codex" as const,
          externalId: "codex-native",
          modeId: "plan",
          configValues: {},
          seenSequence: 2,
        },
        {
          id: 2,
          agentType: "pi" as const,
          externalId: "pi-native",
          modeId: "high",
          configValues: { model: "pi-provider/pi-model" },
          seenSequence: 4,
        },
      ],
    }
    api.switch.mockResolvedValue(pi)
    const { result } = renderHook(() => useBusinessConversation(1))
    await waitFor(() => expect(result.current.state).not.toBeNull())
    await act(async () => {
      await result.current.switchAgent("pi")
    })
    expect(api.switch).toHaveBeenCalledWith(
      expect.objectContaining({
        targetAgent: "pi",
        preferredModeId: "high",
        preferredConfigValues: {
          model: "pi-provider/pi-model",
          thought_level: "high",
        },
      })
    )
    api.get.mockResolvedValue(pi)
    await act(async () => {
      await result.current.refresh()
    })
    expect(result.current.state).toEqual(pi)
    expect(api.switch).toHaveBeenCalledTimes(1)
  })
  it("sends only the selected engine's saved settings", async () => {
    localStorage.setItem(
      "codeg:selector-prefs",
      JSON.stringify({
        codex: {
          modeId: "plan",
          configValues: { model: "codex-model", effort: "high" },
        },
        claude_code: {
          modeId: "default",
          configValues: { model: "claude-model" },
        },
      })
    )
    api.switch.mockResolvedValue(state(1, 3, "claude_code"))
    const { result } = renderHook(() => useBusinessConversation(1))
    await waitFor(() => expect(result.current.state).not.toBeNull())
    await act(async () => {
      await result.current.switchAgent("claude_code")
    })
    expect(api.switch).toHaveBeenCalledWith(
      expect.objectContaining({
        preferredModeId: "default",
        preferredConfigValues: { model: "claude-model" },
      })
    )
  })
  it("keeps the active engine until committed and coalesces repeated clicks", async () => {
    let resolve!: (value: BusinessConversationState) => void
    api.switch.mockReturnValue(
      new Promise((done) => {
        resolve = done
      })
    )
    const { result } = renderHook(() => useBusinessConversation(1))
    await waitFor(() => expect(result.current.state?.activeAgent).toBe("codex"))
    let pending!: Promise<void>
    act(() => {
      pending = result.current.switchAgent("claude_code")
      void result.current.switchAgent("claude_code")
    })
    await waitFor(() => expect(api.switch).toHaveBeenCalledTimes(1))
    expect(result.current.pending).toBe(true)
    expect(result.current.state?.activeAgent).toBe("codex")
    await act(async () => {
      resolve({ ...state(1, 3, "claude_code"), switchStatus: "succeeded" })
      await pending
    })
    expect(result.current.state?.activeAgent).toBe("claude_code")
    expect(result.current.pending).toBe(false)
    expect(api.switch.mock.calls[0][0]).toMatchObject({
      conversationId: 1,
      targetAgent: "claude_code",
      expectedRevision: 0,
    })
  })
  it("does not let a late response retarget a different business", async () => {
    let resolve!: (value: BusinessConversationState) => void
    api.switch.mockReturnValue(
      new Promise((done) => {
        resolve = done
      })
    )
    const { result, rerender } = renderHook(
      ({ cid }) => useBusinessConversation(cid),
      { initialProps: { cid: 1 } }
    )
    await waitFor(() => expect(result.current.state?.conversationId).toBe(1))
    let pending!: Promise<void>
    act(() => {
      pending = result.current.switchAgent("claude_code")
    })
    api.get.mockResolvedValue(state(2))
    rerender({ cid: 2 })
    await waitFor(() => expect(result.current.state?.conversationId).toBe(2))
    await act(async () => {
      resolve(state(1, 9, "claude_code"))
      await pending
    })
    expect(result.current.state?.activeAgent).toBe("codex")
    expect(result.current.state?.conversationId).toBe(2)
  })
  it("rejects older generations after another client commits a switch", async () => {
    api.get.mockResolvedValue(state(1, 5, "claude_code"))
    const { result } = renderHook(() => useBusinessConversation(1))
    await waitFor(() => expect(result.current.state?.revision).toBe(5))
    api.get.mockResolvedValue(state(1, 2, "codex"))
    await act(async () => {
      await result.current.refresh()
    })
    expect(result.current.state?.revision).toBe(5)
    expect(result.current.state?.activeAgent).toBe("claude_code")
  })
  it("refreshes a stale task revision before submitting the same handoff intent", async () => {
    api.get.mockResolvedValue(state(1, 3, "claude_code"))
    api.switch.mockResolvedValue(state(1, 5, "codex"))
    const { result } = renderHook(() => useBusinessConversation(1))
    await waitFor(() => expect(result.current.state?.revision).toBe(3))
    // Another client edits shared task state before the next background poll.
    api.get.mockResolvedValue(state(1, 4, "claude_code"))
    await act(async () => {
      await result.current.switchAgent("codex")
    })
    expect(api.switch).toHaveBeenCalledWith(
      expect.objectContaining({ targetAgent: "codex", expectedRevision: 4 })
    )
    expect(result.current.error).toBeNull()
  })
  it("does not overwrite another client's switch discovered before admission", async () => {
    const { result } = renderHook(() => useBusinessConversation(1))
    await waitFor(() => expect(result.current.state?.revision).toBe(0))
    api.get.mockResolvedValue({
      ...state(1, 1),
      pendingRequestId: "other-client",
      pendingAgent: "claude_code",
      switchStatus: "queued",
    })
    api.switch.mockResolvedValue(state(1, 2, "claude_code"))
    await act(async () => {
      await result.current.switchAgent("claude_code")
    })
    expect(api.switch).not.toHaveBeenCalled()
    expect(result.current.state?.pendingRequestId).toBe("other-client")
    expect(result.current.error).toContain("changed")
  })
  it("shows structured rejection detail without retrying an unaccepted switch", async () => {
    api.switch.mockRejectedValue({
      code: "task_execution_failed",
      message: "Task execution failed",
      detail: "Conversation changed; refresh its agent state and retry",
    })
    const { result } = renderHook(() => useBusinessConversation(1))
    await waitFor(() => expect(result.current.state).not.toBeNull())
    await act(async () => {
      await result.current.switchAgent("claude_code")
    })
    expect(api.switch).toHaveBeenCalledTimes(1)
    expect(result.current.error).toBe(
      "Conversation changed; refresh its agent state and retry"
    )
    expect(result.current.state?.activeAgent).toBe("codex")
  })
  it("does not submit from an old snapshot when the admission read fails", async () => {
    const { result } = renderHook(() => useBusinessConversation(1))
    await waitFor(() => expect(result.current.state).not.toBeNull())
    api.get.mockRejectedValueOnce(new Error("Server unavailable"))
    await act(async () => {
      await result.current.switchAgent("claude_code")
    })
    expect(api.switch).not.toHaveBeenCalled()
    expect(result.current.error).toBe("Server unavailable")
    expect(result.current.state?.activeAgent).toBe("codex")
    expect(result.current.pending).toBe(false)
  })
  it("preserves the original engine and provides a retry error after preparation fails", async () => {
    api.switch.mockRejectedValue(
      new Error("Claude authentication is unavailable")
    )
    const { result } = renderHook(() => useBusinessConversation(1))
    await waitFor(() => expect(result.current.state).not.toBeNull())
    await act(async () => {
      await result.current.switchAgent("claude_code")
    })
    expect(result.current.state?.activeAgent).toBe("codex")
    expect(result.current.error).toContain("authentication")
    expect(result.current.pending).toBe(false)
  })
  it("leaves a new draft ready for its normal first send", () => {
    const { result } = renderHook(() => useBusinessConversation(null))
    expect(result.current.pending).toBe(false)
    expect(api.get).not.toHaveBeenCalled()
  })
  it("does not poll hidden business conversations", () => {
    renderHook(() => useBusinessConversation(1, false))
    expect(api.get).not.toHaveBeenCalled()
  })
})
