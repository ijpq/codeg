import { afterEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({ call: vi.fn() }))

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call: mocks.call }),
  getShellTransport: () => ({ call: vi.fn() }),
  isDesktop: () => false,
  isRemoteDesktopMode: () => false,
  getActiveRemoteConnectionId: () => null,
  notifyRemoteDesktopUnauthorized: vi.fn(),
}))

import { createConversationBranch } from "./api"

afterEach(() => {
  vi.useRealTimers()
  mocks.call.mockReset()
})

describe("durable branch creation task polling", () => {
  it("survives a timed-out status request and returns the later success", async () => {
    vi.useFakeTimers()
    const result = {
      branchConversationId: 19,
      sourceConversationId: 7,
      folderId: 3,
      connectionId: null,
      branchSessionId: null,
      sessionReady: false,
      promptReady: false,
      lifecycleState: "provisional",
      forkMode: "snapshot",
      inheritanceMode: "structured_snapshot",
      inheritedMessageCount: 25,
      inheritanceTruncated: false,
      fallbackReason: null,
    }
    mocks.call
      .mockResolvedValueOnce({
        requestId: "stable-op",
        sourceConversationId: 7,
        status: "running",
        stage: "creating_branch",
      })
      .mockRejectedValueOnce(new Error("request timed out"))
      .mockResolvedValueOnce({
        requestId: "stable-op",
        sourceConversationId: 7,
        status: "succeeded",
        stage: "completed",
        result,
      })

    const pending = createConversationBranch({
      requestId: "stable-op",
      operationId: "stable-op",
      sourceConversationId: 7,
    })
    await vi.advanceTimersByTimeAsync(1_000)
    await vi.advanceTimersByTimeAsync(1_000)
    await expect(pending).resolves.toEqual(result)
    expect(mocks.call).toHaveBeenNthCalledWith(
      1,
      "queue_conversation_branch_creation",
      expect.anything(),
      { timeoutMs: 30_000 }
    )
    expect(mocks.call).toHaveBeenCalledTimes(3)
  })
})
