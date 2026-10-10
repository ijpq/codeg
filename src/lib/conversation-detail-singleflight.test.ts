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

  it("starts a reload after an older default read without abandoning its consumer", async () => {
    let finishOld!: (value: typeof detail) => void
    const latest = { ...detail, turns: [{ id: "new-answer" }] }
    mocks.call
      .mockReturnValueOnce(
        new Promise((resolve) => {
          finishOld = resolve
        })
      )
      .mockResolvedValueOnce(latest)
    const old = getFolderConversation(26, { userTurnLimit: 25 })
    const fresh = getFolderConversation(26, {
      userTurnLimit: 25,
      cacheMode: "reload",
    })

    await expect(fresh).resolves.toEqual(latest)
    expect(mocks.call).toHaveBeenCalledTimes(2)
    expect(mocks.call.mock.calls[0]![2].signal.aborted).toBe(false)
    finishOld(detail)
    await expect(old).resolves.toEqual(detail)
  })

  it("shares a refresh generation but isolates a later generation", async () => {
    const resolves: Array<(value: typeof detail) => void> = []
    mocks.call.mockImplementation(
      () => new Promise((resolve) => resolves.push(resolve))
    )
    const request = (generation: number) =>
      getFolderConversation(26, {
        userTurnLimit: 25,
        cacheMode: "reload",
        requestGeneration: generation,
      })
    const first = request(1)
    const shared = request(1)
    const next = request(2)

    expect(mocks.call).toHaveBeenCalledTimes(2)
    resolves[1]!(detail)
    await next
    resolves[0]!(detail)
    await Promise.all([first, shared])
  })

  it.each([26, undefined])(
    "invalidation (%s) prevents an old flight from refilling the cache",
    async (id) => {
      vi.stubEnv("NODE_ENV", "production")
      let finishOld!: (value: typeof detail) => void
      const latest = { ...detail, turns: [{ id: "new-answer" }] }
      mocks.call
        .mockReturnValueOnce(
          new Promise((resolve) => {
            finishOld = resolve
          })
        )
        .mockResolvedValueOnce(latest)
      const old = getFolderConversation(26, { userTurnLimit: 25 })
      invalidateFolderConversationCache(id)
      finishOld(detail)
      await old

      await expect(
        getFolderConversation(26, { userTurnLimit: 25 })
      ).resolves.toEqual(latest)
      expect(mocks.call).toHaveBeenCalledTimes(2)
    }
  )

  it("an older generation finishing last cannot replace the newest cached tail", async () => {
    vi.stubEnv("NODE_ENV", "production")
    let finishOld!: (value: typeof detail) => void
    const latest = { ...detail, turns: [{ id: "new-answer" }] }
    mocks.call
      .mockReturnValueOnce(
        new Promise((resolve) => {
          finishOld = resolve
        })
      )
      .mockResolvedValueOnce(latest)
    const old = getFolderConversation(26, {
      userTurnLimit: 25,
      requestGeneration: 1,
      cacheMode: "reload",
    })
    await getFolderConversation(26, {
      userTurnLimit: 25,
      requestGeneration: 2,
      cacheMode: "reload",
    })
    finishOld(detail)
    await old

    await expect(
      getFolderConversation(26, { userTurnLimit: 25 })
    ).resolves.toEqual(latest)
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

  it("does not cache an incomplete tail while the index can still recover", async () => {
    vi.stubEnv("NODE_ENV", "production")
    const partial = {
      ...detail,
      history_page: { ...detail.history_page, page_complete: false },
    }
    mocks.call.mockResolvedValueOnce(partial).mockResolvedValueOnce(detail)
    await expect(
      getFolderConversation(26, { userTurnLimit: 25 })
    ).resolves.toEqual(partial)
    await expect(
      getFolderConversation(26, { userTurnLimit: 25 })
    ).resolves.toEqual(detail)
    expect(mocks.call).toHaveBeenCalledTimes(2)
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
