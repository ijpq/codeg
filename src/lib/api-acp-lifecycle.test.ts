import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  call: vi.fn(),
}))

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call: mocks.call }),
  getShellTransport: () => ({ call: vi.fn() }),
  isDesktop: () => false,
  isRemoteDesktopMode: () => false,
  getActiveRemoteConnectionId: () => null,
  notifyRemoteDesktopUnauthorized: vi.fn(),
}))

import { acpCancel, acpDisconnect } from "@/lib/api"

describe("ACP lifecycle request audit metadata", () => {
  beforeEach(() => {
    mocks.call.mockReset()
    mocks.call.mockResolvedValue(undefined)
  })

  it("marks the only frontend cancel entry point as an explicit user stop", async () => {
    await acpCancel("conn-1")

    expect(mocks.call).toHaveBeenCalledWith(
      "acp_cancel",
      expect.objectContaining({
        connectionId: "conn-1",
        requestSource: "user_stop",
        frontendGeneration: expect.any(Number),
      })
    )
  })

  it("marks a destructive frontend connection teardown separately", async () => {
    await acpDisconnect("conn-2")

    expect(mocks.call).toHaveBeenCalledWith(
      "acp_disconnect",
      expect.objectContaining({
        connectionId: "conn-2",
        requestSource: "frontend_disconnect",
        frontendGeneration: expect.any(Number),
      })
    )
  })

  it("uses a monotonically increasing generation across lifecycle requests", async () => {
    await acpCancel("conn-3")
    await acpDisconnect("conn-3")

    const first = mocks.call.mock.calls[0]![1] as {
      frontendGeneration: number
    }
    const second = mocks.call.mock.calls[1]![1] as {
      frontendGeneration: number
    }
    expect(second.frontendGeneration).toBeGreaterThan(first.frontendGeneration)
  })
})
