import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({ call: vi.fn() }))

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call: mocks.call }),
  getShellTransport: () => ({ call: vi.fn() }),
  isDesktop: () => false,
  isRemoteDesktopMode: () => false,
  getActiveRemoteConnectionId: () => null,
  notifyRemoteDesktopUnauthorized: vi.fn(),
}))

import { getFolderConversation } from "@/lib/api"

const detail = {
  summary: { id: 26 },
  turns: [],
  artifact_runs: [],
  deliverables: [],
  deliverable_runs: [],
  history_page: { has_more: false, loaded_turns: 0 },
}

describe("conversation detail request single-flight", () => {
  beforeEach(() => {
    mocks.call.mockReset()
  })

  it("reuses concurrent requests for the same conversation and cursor", async () => {
    let resolve!: (value: typeof detail) => void
    mocks.call.mockReturnValue(
      new Promise<typeof detail>((done) => {
        resolve = done
      })
    )

    const first = getFolderConversation(26, {
      beforeCursor: null,
      userTurnLimit: 6,
    })
    const second = getFolderConversation(26, {
      beforeCursor: null,
      userTurnLimit: 6,
    })

    expect(mocks.call).toHaveBeenCalledTimes(1)
    resolve(detail)
    await expect(Promise.all([first, second])).resolves.toEqual([
      detail,
      detail,
    ])
  })

  it("keeps different cursors as independent pages", async () => {
    mocks.call.mockResolvedValue(detail)
    await Promise.all([
      getFolderConversation(26, { beforeCursor: null, userTurnLimit: 6 }),
      getFolderConversation(26, {
        beforeCursor: "codex:123",
        userTurnLimit: 6,
      }),
    ])
    expect(mocks.call).toHaveBeenCalledTimes(2)
  })
})
