import { beforeEach, describe, expect, it, vi } from "vitest"
import type { DbConversationDetail, MessageTurn } from "@/lib/types"
import {
  HISTORY_PAGE_USER_TURNS,
  resetConversationRuntimeStore,
  useConversationRuntimeStore,
} from "@/stores/conversation-runtime-store"

vi.mock("@/lib/api", () => ({
  getFolderConversation: vi.fn(),
  invalidateFolderConversationCache: vi.fn(),
  listConversationOutputWindow: vi.fn(),
}))

const { getFolderConversation, listConversationOutputWindow } =
  await import("@/lib/api")
const mockGet = vi.mocked(getFolderConversation)
const mockOutputs = vi.mocked(listConversationOutputWindow)
const CONVERSATION_ID = 16

function turn(id: string, role: "user" | "assistant"): MessageTurn {
  return {
    id,
    role,
    blocks: [{ type: "text", text: id }],
    timestamp: `2026-08-06T00:00:0${id.length}.000Z`,
  }
}

function page(
  turns: MessageTurn[],
  nextCursor: string | null
): DbConversationDetail {
  return {
    summary: {
      id: CONVERSATION_ID,
      folder_id: 1,
      title: "large history",
      title_locked: false,
      agent_type: "codex",
      status: "completed",
      kind: "regular",
      model: "gpt-5.5",
      git_branch: null,
      external_id: "session-16",
      message_count: 401,
      child_count: 0,
      created_at: "2026-08-06T00:00:00.000Z",
      updated_at: "2026-08-06T00:00:00.000Z",
      pinned_at: null,
    },
    turns,
    session_stats: null,
    history_page: {
      next_cursor: nextCursor,
      has_more: nextCursor !== null,
      loaded_turns: turns.length,
    },
  }
}

beforeEach(() => {
  resetConversationRuntimeStore()
  mockGet.mockReset()
  mockOutputs.mockReset()
})

describe("conversation history pagination", () => {
  it("loads the bounded tail, prepends older turns once, and keeps order", async () => {
    mockGet.mockResolvedValueOnce(
      page([turn("u3", "user"), turn("a3", "assistant")], "codex:300")
    )
    useConversationRuntimeStore.getState().actions.fetchDetail(CONVERSATION_ID)
    await vi.waitFor(() => {
      expect(
        useConversationRuntimeStore
          .getState()
          .byConversationId.get(CONVERSATION_ID)?.detailLoading
      ).toBe(false)
    })
    expect(mockGet).toHaveBeenNthCalledWith(
      1,
      CONVERSATION_ID,
      expect.objectContaining({
        userTurnLimit: HISTORY_PAGE_USER_TURNS,
        requestGeneration: expect.any(Number),
        signal: expect.any(AbortSignal),
      })
    )

    let resolveOlder!: (value: DbConversationDetail) => void
    mockGet.mockImplementationOnce(
      () =>
        new Promise<DbConversationDetail>((resolve) => {
          resolveOlder = resolve
        })
    )
    const actions = useConversationRuntimeStore.getState().actions
    const first = actions.loadEarlierHistory(CONVERSATION_ID)
    const duplicate = actions.loadEarlierHistory(CONVERSATION_ID)
    expect(first).toBe(duplicate)
    expect(mockGet).toHaveBeenNthCalledWith(
      2,
      CONVERSATION_ID,
      expect.objectContaining({
        beforeCursor: "codex:300",
        userTurnLimit: HISTORY_PAGE_USER_TURNS,
        requestGeneration: expect.any(Number),
        signal: expect.any(AbortSignal),
      })
    )

    // Include one overlapping id to prove a retried/duplicated page cannot
    // duplicate a message in the visible timeline.
    resolveOlder(
      page(
        [turn("u2", "user"), turn("a2", "assistant"), turn("u3", "user")],
        "codex:200"
      )
    )
    await first

    const session = useConversationRuntimeStore
      .getState()
      .byConversationId.get(CONVERSATION_ID)
    expect(session?.detail?.turns.map(({ id }) => id)).toEqual([
      "u2",
      "a2",
      "u3",
      "a3",
    ])
    expect(session?.detail?.history_page?.next_cursor).toBe("codex:200")
    expect(session?.historyPageError).toBeNull()
  })

  it("retains loaded messages on failure and exposes a retry", async () => {
    mockGet
      .mockResolvedValueOnce(
        page([turn("u3", "user"), turn("a3", "assistant")], "codex:300")
      )
      .mockRejectedValueOnce(new Error("network interrupted"))
      .mockResolvedValueOnce(
        page([turn("u2", "user"), turn("a2", "assistant")], null)
      )
    const actions = useConversationRuntimeStore.getState().actions
    actions.fetchDetail(CONVERSATION_ID)
    await vi.waitFor(() => {
      expect(
        useConversationRuntimeStore
          .getState()
          .byConversationId.get(CONVERSATION_ID)?.detailLoading
      ).toBe(false)
    })

    await actions.loadEarlierHistory(CONVERSATION_ID)
    let session = useConversationRuntimeStore
      .getState()
      .byConversationId.get(CONVERSATION_ID)
    expect(session?.detail?.turns.map(({ id }) => id)).toEqual(["u3", "a3"])
    expect(session?.historyPageError).toContain("network interrupted")

    await actions.loadEarlierHistory(CONVERSATION_ID)
    session = useConversationRuntimeStore
      .getState()
      .byConversationId.get(CONVERSATION_ID)
    expect(session?.detail?.turns.map(({ id }) => id)).toEqual([
      "u2",
      "a2",
      "u3",
      "a3",
    ])
    expect(session?.detail?.history_page?.has_more).toBe(false)
    expect(session?.historyPageError).toBeNull()
  })

  it("refreshes artifact and deliverable state without reloading history", async () => {
    mockGet.mockResolvedValueOnce(
      page([turn("u3", "user"), turn("a3", "assistant")], "codex:300")
    )
    const actions = useConversationRuntimeStore.getState().actions
    actions.fetchDetail(CONVERSATION_ID)
    await vi.waitFor(() => {
      expect(
        useConversationRuntimeStore
          .getState()
          .byConversationId.get(CONVERSATION_ID)?.detailLoading
      ).toBe(false)
    })
    mockOutputs.mockResolvedValueOnce({
      artifact_runs: [],
      deliverable_runs: [
        {
          turn_run_id: "run-3",
          conversation_id: CONVERSATION_ID,
          client_message_id: "u3",
          user_turn_id: "u3",
          started_at: "2026-08-06T00:00:00.000Z",
          completed_at: "2026-08-06T00:00:01.000Z",
          deliverables: [],
        },
      ],
    })

    await actions.refreshVisibleOutputs(CONVERSATION_ID)

    expect(mockGet).toHaveBeenCalledTimes(1)
    expect(mockOutputs).toHaveBeenCalledWith(
      CONVERSATION_ID,
      expect.arrayContaining([expect.objectContaining({ id: "u3" })]),
      undefined
    )
    expect(
      useConversationRuntimeStore
        .getState()
        .byConversationId.get(CONVERSATION_ID)?.detail?.deliverable_runs
    ).toHaveLength(1)
  })

  it("retains fingerprint identities across pages and retries a late declaration event", async () => {
    const latest = page([turn("u3", "user"), turn("a3", "assistant")], "cursor")
    latest.output_turn_refs = [
      {
        id: "u3",
        timestamp: latest.turns[0].timestamp,
        is_user: true,
        prompt_fingerprint: "fp3",
      },
    ]
    mockGet.mockResolvedValueOnce(latest)
    const actions = useConversationRuntimeStore.getState().actions
    actions.fetchDetail(CONVERSATION_ID)
    await vi.waitFor(() =>
      expect(
        useConversationRuntimeStore
          .getState()
          .byConversationId.get(CONVERSATION_ID)?.detailLoading
      ).toBe(false)
    )
    const older = page([turn("u2", "user"), turn("a2", "assistant")], null)
    older.output_turn_refs = [
      {
        id: "u2",
        timestamp: older.turns[0].timestamp,
        is_user: true,
        prompt_fingerprint: "fp2",
      },
    ]
    mockGet.mockResolvedValueOnce(older)
    await actions.loadEarlierHistory(CONVERSATION_ID)
    let resolveFirst!: (value: {
      artifact_runs: []
      deliverable_runs: []
    }) => void
    mockOutputs.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveFirst = resolve
        })
    )
    mockOutputs.mockResolvedValueOnce({
      artifact_runs: [],
      deliverable_runs: [
        {
          turn_run_id: "late-old-run",
          conversation_id: CONVERSATION_ID,
          user_turn_id: "u2",
          client_message_id: "client-u2",
          started_at: older.turns[0].timestamp,
          deliverables: [],
        },
      ],
    })
    const flight = actions.refreshVisibleOutputs(CONVERSATION_ID)
    void actions.refreshVisibleOutputs(CONVERSATION_ID)
    resolveFirst({ artifact_runs: [], deliverable_runs: [] })
    await flight
    await vi.waitFor(() => expect(mockOutputs).toHaveBeenCalledTimes(2))
    expect(
      mockOutputs.mock.calls[1][2]?.map((ref) => ref.prompt_fingerprint).sort()
    ).toEqual(["fp2", "fp3"])
    expect(
      useConversationRuntimeStore
        .getState()
        .byConversationId.get(CONVERSATION_ID)?.detail?.deliverable_runs?.[0]
        .user_turn_id
    ).toBe("u2")
    expect(mockGet).toHaveBeenCalledTimes(2)
  })

  it("does not let an old output response overwrite a newer history snapshot", async () => {
    mockGet.mockResolvedValueOnce(page([turn("u3", "user")], null))
    const actions = useConversationRuntimeStore.getState().actions
    actions.fetchDetail(CONVERSATION_ID)
    await vi.waitFor(() =>
      expect(
        useConversationRuntimeStore
          .getState()
          .byConversationId.get(CONVERSATION_ID)?.detailLoading
      ).toBe(false)
    )
    let resolveFirst!: (value: {
      artifact_runs: []
      deliverable_runs: []
    }) => void
    mockOutputs.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveFirst = resolve
        })
    )
    mockOutputs.mockResolvedValueOnce({
      artifact_runs: [],
      deliverable_runs: [],
    })
    const first = actions.refreshVisibleOutputs(CONVERSATION_ID)
    mockGet.mockResolvedValueOnce(page([turn("u4", "user")], null))
    actions.refetchDetail(CONVERSATION_ID)
    await vi.waitFor(() =>
      expect(
        useConversationRuntimeStore
          .getState()
          .byConversationId.get(CONVERSATION_ID)?.detail?.turns[0].id
      ).toBe("u4")
    )
    resolveFirst({ artifact_runs: [], deliverable_runs: [] })
    await first
    await vi.waitFor(() => expect(mockOutputs).toHaveBeenCalledTimes(2))
    expect(mockOutputs.mock.calls[1][1][0].id).toBe("u4")
  })
})
