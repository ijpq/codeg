import { act, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { SessionGoalState } from "@/lib/api"
import { useNativeGoalTranscript } from "./use-native-goal-transcript"

const runtime = vi.hoisted(() => ({
  byConversationId: new Map<
    number,
    {
      detailLoading: boolean
      loadingOlderTurns: boolean
      activeTurnToken: string | null
      pendingOutOfTurnContent: boolean
      detailError: string | null
    }
  >(),
  actions: { refetchDetail: vi.fn() },
}))
vi.mock("@/stores/conversation-runtime-store", () => ({
  useConversationRuntimeStore: { getState: () => runtime },
}))

const snapshot = (status: string | null): SessionGoalState => ({
  goal: status ? { status, objective: "Real native work" } : null,
  actions: ["set", "pause", "resume", "clear"],
})
const advance = async (ms: number) => {
  await act(async () => vi.advanceTimersByTimeAsync(ms))
}
const session = () => runtime.byConversationId.get(42)!

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(new Date("2026-10-16T00:00:00Z"))
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible")
  runtime.byConversationId.clear()
  runtime.byConversationId.set(42, {
    detailLoading: false,
    loadingOlderTurns: false,
    activeTurnToken: null,
    pendingOutOfTurnContent: true,
    detailError: null,
  })
  runtime.actions.refetchDetail.mockReset().mockImplementation((id: number) => {
    runtime.byConversationId.get(id)!.pendingOutOfTurnContent = false
  })
})
afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe("native Goal transcript projection", () => {
  it("leaves unrelated out-of-turn content on its existing manual path", async () => {
    const { result } = renderHook(() => useNativeGoalTranscript("c", 42, true))
    act(() => result.current(snapshot(null)))
    await advance(60_000)
    expect(runtime.actions.refetchDetail).not.toHaveBeenCalled()
    expect(session().pendingOutOfTurnContent).toBe(true)
  })

  it("shows Goal output through the bounded reader and coalesces token bursts", async () => {
    const { result } = renderHook(() => useNativeGoalTranscript("c", 42, true))
    act(() => result.current(snapshot("active")))
    await advance(4000)
    expect(runtime.actions.refetchDetail).toHaveBeenCalledWith(42, {
      preserveLive: true,
    })
    runtime.actions.refetchDetail.mockClear()
    // Repeated status polls must not restart the settling reads.
    act(() => {
      for (let i = 0; i < 100; i++) result.current(snapshot("active"))
    })
    await advance(60_000)
    expect(runtime.actions.refetchDetail).not.toHaveBeenCalled()
    for (let i = 0; i < 100; i++) session().pendingOutOfTurnContent = true
    await advance(2000)
    expect(runtime.actions.refetchDetail).toHaveBeenCalledTimes(1)
  })

  it.each(["paused", "complete", null])(
    "drains trailing persisted records after native %s without further deltas",
    async (status) => {
      const { result } = renderHook(() =>
        useNativeGoalTranscript("c", 42, true)
      )
      act(() => result.current(snapshot("active")))
      await advance(4000)
      runtime.actions.refetchDetail.mockClear()
      act(() => result.current(snapshot(status)))
      await advance(4000)
      expect(runtime.actions.refetchDetail).toHaveBeenCalledTimes(2)
      runtime.actions.refetchDetail.mockClear()
      await advance(60_000)
      expect(runtime.actions.refetchDetail).not.toHaveBeenCalled()
    }
  )

  it.each(["detailLoading", "loadingOlderTurns", "activeTurnToken"] as const)(
    "waits for %s without cancelling or replacing the foreground work",
    async (busy) => {
      if (busy === "activeTurnToken") session().activeTurnToken = "user-turn"
      else session()[busy] = true
      const { result } = renderHook(() =>
        useNativeGoalTranscript("c", 42, true)
      )
      act(() => result.current(snapshot("active")))
      await advance(5000)
      expect(runtime.actions.refetchDetail).not.toHaveBeenCalled()
      if (busy === "activeTurnToken") session().activeTurnToken = null
      else session()[busy] = false
      await advance(2000)
      expect(runtime.actions.refetchDetail).toHaveBeenCalledTimes(1)
    }
  )

  it("waits for a visible active surface", async () => {
    const visibility = vi.mocked(
      Object.getOwnPropertyDescriptor(document, "visibilityState")!.get!
    )
    visibility.mockReturnValue("hidden")
    const { result, rerender } = renderHook(
      ({ active }) => useNativeGoalTranscript("c", 42, active),
      { initialProps: { active: false } }
    )
    act(() => result.current(snapshot("active")))
    rerender({ active: true })
    await advance(10_000)
    expect(runtime.actions.refetchDetail).not.toHaveBeenCalled()
    visibility.mockReturnValue("visible")
    act(() => document.dispatchEvent(new Event("visibilitychange")))
    await advance(500)
    expect(runtime.actions.refetchDetail).toHaveBeenCalledTimes(1)
  })

  it("does not read a negative draft id, then follows its durable handoff", async () => {
    const { result, rerender } = renderHook(
      ({ id }) => useNativeGoalTranscript("c", id, true),
      { initialProps: { id: -9 } }
    )
    act(() => result.current(snapshot("active")))
    await advance(5000)
    expect(runtime.actions.refetchDetail).not.toHaveBeenCalled()
    rerender({ id: 42 })
    await advance(500)
    expect(runtime.actions.refetchDetail).toHaveBeenCalledWith(42, {
      preserveLive: true,
    })
  })

  it("rejects an old connection's late snapshot", async () => {
    const { result, rerender } = renderHook(
      ({ id }) => useNativeGoalTranscript(id, 42, true),
      { initialProps: { id: "old" } }
    )
    const stale = result.current
    rerender({ id: "new" })
    act(() => stale(snapshot("active")))
    await advance(5000)
    expect(runtime.actions.refetchDetail).not.toHaveBeenCalled()
    act(() => result.current(snapshot("active")))
    await advance(500)
    expect(runtime.actions.refetchDetail).toHaveBeenCalledTimes(1)
  })

  it("backs off failed reads without losing the pending content", async () => {
    session().detailError = "unreadable"
    runtime.actions.refetchDetail.mockImplementation(() => {})
    const { result } = renderHook(() => useNativeGoalTranscript("c", 42, true))
    act(() => result.current(snapshot("active")))
    await advance(4000)
    expect(runtime.actions.refetchDetail).toHaveBeenCalledTimes(1)
    await advance(4000)
    expect(runtime.actions.refetchDetail).toHaveBeenCalledTimes(2)
    expect(session().pendingOutOfTurnContent).toBe(true)
  })

  it("cancels pending reads when the surface unmounts", async () => {
    const { result, unmount } = renderHook(() =>
      useNativeGoalTranscript("c", 42, true)
    )
    act(() => result.current(snapshot("active")))
    unmount()
    await advance(60_000)
    expect(runtime.actions.refetchDetail).not.toHaveBeenCalled()
  })
})
