import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({ call: vi.fn() }))

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call: mocks.call }),
  getShellTransport: () => ({ call: vi.fn() }),
  isDesktop: () => false,
  isRemoteDesktopMode: () => false,
  getActiveRemoteConnectionId: () => null,
  notifyRemoteDesktopUnauthorized: vi.fn(),
}))

import {
  getFolderConversation,
  invalidateFolderConversationCache,
} from "@/lib/api"

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
    invalidateFolderConversationCache()
  })

  afterEach(() => {
    vi.unstubAllEnvs()
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

  it("reuses a recently completed first screen from the bounded LRU", async () => {
    vi.stubEnv("NODE_ENV", "production")
    mocks.call.mockResolvedValue(detail)
    await getFolderConversation(226, {
      beforeCursor: null,
      userTurnLimit: 25,
    })
    await getFolderConversation(226, {
      beforeCursor: null,
      userTurnLimit: 25,
    })
    expect(mocks.call).toHaveBeenCalledTimes(1)
  })

  it("cancels shared backend work only after every consumer detaches", async () => {
    mocks.call.mockImplementation(
      (_command: string, _args: unknown, options?: { signal?: AbortSignal }) =>
        new Promise((_resolve, reject) => {
          options?.signal?.addEventListener(
            "abort",
            () => {
              const error = new Error("cancelled")
              error.name = "AbortError"
              reject(error)
            },
            { once: true }
          )
        })
    )
    const firstController = new AbortController()
    const secondController = new AbortController()
    const first = getFolderConversation(126, {
      beforeCursor: null,
      userTurnLimit: 25,
      signal: firstController.signal,
    })
    const second = getFolderConversation(126, {
      beforeCursor: null,
      userTurnLimit: 25,
      signal: secondController.signal,
    })

    firstController.abort()
    await expect(first).rejects.toMatchObject({ name: "AbortError" })
    const transportSignal = mocks.call.mock.calls[0]![2]?.signal as AbortSignal
    expect(transportSignal.aborted).toBe(false)
    secondController.abort()
    await expect(second).rejects.toMatchObject({ name: "AbortError" })
    expect(transportSignal.aborted).toBe(true)
    expect(mocks.call).toHaveBeenCalledTimes(1)
  })

  it("starts fresh instead of reusing a transport flight already cancelled by its last consumer", async () => {
    mocks.call
      .mockImplementationOnce(
        (
          _command: string,
          _args: unknown,
          options?: { signal?: AbortSignal }
        ) =>
          new Promise((_resolve, reject) => {
            options?.signal?.addEventListener(
              "abort",
              () => {
                const error = new Error("cancelled")
                error.name = "AbortError"
                reject(error)
              },
              { once: true }
            )
          })
      )
      .mockResolvedValueOnce(detail)

    const abandoned = new AbortController()
    const first = getFolderConversation(326, {
      userTurnLimit: 25,
      signal: abandoned.signal,
    })
    abandoned.abort()
    const replacement = getFolderConversation(326, { userTurnLimit: 25 })

    await expect(first).rejects.toMatchObject({ name: "AbortError" })
    await expect(replacement).resolves.toEqual(detail)
    expect(mocks.call).toHaveBeenCalledTimes(2)
  })
})
