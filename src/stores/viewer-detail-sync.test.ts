/**
 * Coverage for `syncViewerDetail` — the cross-client fix for "a short reply
 * completed on another client never appears here".
 *
 * A client that is only VIEWING a conversation (another client owns the live
 * agent) has no live promotion path for a completed turn: the panel promotes on
 * the connection's `prompting → connected` edge, which a viewer that missed the
 * short live stream never observes, and the global `conversation://changed`
 * side-channel only patches the sidebar — not the open detail. `syncViewerDetail`
 * closes that gap by polling the persisted transcript, and:
 *   - refuses to touch an OWNER (its in-memory reply is fresher than any disk
 *     read that could race the transcript flush), and
 *   - keeps polling while the transcript still ends at the user prompt (the
 *     reply hasn't been flushed yet), stopping once the assistant reply lands.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { DbConversationDetail, MessageTurn } from "@/lib/types"
import type { LiveMessage } from "@/contexts/acp-connections-context"
import {
  HISTORY_PAGE_USER_TURNS,
  resetConversationRuntimeStore,
  useConversationRuntimeStore,
  type ConversationRuntimeSession,
} from "@/stores/conversation-runtime-store"

vi.mock("@/lib/api", () => ({
  getFolderConversation: vi.fn(),
  invalidateFolderConversationCache: vi.fn(),
  listConversationOutputWindow: vi.fn(),
}))

const { getFolderConversation } = await import("@/lib/api")
const mockGet = vi.mocked(getFolderConversation)

const CID = 99

function userTurn(id: string, text: string): MessageTurn {
  return { id, role: "user", blocks: [{ type: "text", text }], timestamp: "" }
}

function assistantTurn(id: string, text: string): MessageTurn {
  return {
    id,
    role: "assistant",
    blocks: [{ type: "text", text }],
    timestamp: "",
  }
}

function detail(
  turns: MessageTurn[],
  watermark: number | null = null,
  inFlightUserTurnId: string | null = null
): DbConversationDetail {
  return {
    summary: {
      id: CID,
      folder_id: 1,
      agent_type: "claude_code",
      title: null,
      title_locked: false,
      status: "in_progress",
      kind: "regular",
      model: null,
      git_branch: null,
      external_id: "ext-1",
      message_count: turns.length,
      child_count: 0,
      created_at: "2026-07-18T00:00:00.000Z",
      updated_at: "2026-07-18T00:00:00.000Z",
      pinned_at: null,
    },
    turns,
    session_stats: null,
    transcript_watermark: watermark,
    in_flight_user_turn_id: inFlightUserTurnId,
  }
}

const liveMsg: LiveMessage = {
  id: "lm-1",
  role: "assistant",
  content: [],
  startedAt: 0,
}

function emptySession(conversationId: number): ConversationRuntimeSession {
  return {
    conversationId,
    externalId: "ext-1",
    dbConversationId: null,
    detail: null,
    detailLoading: false,
    detailError: null,
    acpLoadError: null,
    localTurns: [],
    backgroundTurns: [],
    pendingBackgroundSettlements: [],
    pendingCancelledTurnReports: null,
    optimisticTurns: [],
    liveMessage: null,
    promptDeliveries: {},
    syncState: "idle",
    activeTurnToken: null,
    lastTurnOwned: false,
    liveOwnsActiveTurn: false,
    delegationKickoffText: null,
    sessionStats: null,
    historyAssistantBaseline: null,
    batchBoundaryIndex: null,
    batchBoundaryPrefixHash: null,
    loadingOlderTurns: false,
    olderTurnsPrependEpoch: 0,
    pendingOutOfTurnContent: false,
    pendingCleanup: false,
  }
}

function seed(overrides: Partial<ConversationRuntimeSession>): void {
  useConversationRuntimeStore.setState({
    byConversationId: new Map([[CID, { ...emptySession(CID), ...overrides }]]),
  })
}

function session(): ConversationRuntimeSession | undefined {
  return useConversationRuntimeStore.getState().byConversationId.get(CID)
}

function sync(): void {
  useConversationRuntimeStore.getState().actions.syncViewerDetail(CID)
}

beforeEach(() => {
  resetConversationRuntimeStore()
  mockGet.mockReset()
})

afterEach(() => {
  vi.useRealTimers()
})

describe("detail hydration — refresh recovery", () => {
  it("restores the active client message and running phase after a cold refresh", async () => {
    const clientMessageId = "optimistic-image-refresh"
    const startedAt = "2026-07-18T00:05:00.000Z"
    mockGet.mockResolvedValue({
      ...detail(
        [userTurn(clientMessageId, "image + text")],
        null,
        clientMessageId
      ),
      artifact_runs: [
        {
          id: "artifact-run-1",
          conversation_id: CID,
          connection_id: "connection-1",
          client_message_id: clientMessageId,
          folder_id: 1,
          root_path: "/tmp/codeg",
          status: "running",
          capture_incomplete: false,
          stop_reason: null,
          started_at: startedAt,
          completed_at: null,
          changes: [],
        },
      ],
    })

    useConversationRuntimeStore.getState().actions.fetchDetail(CID)
    await vi.waitFor(() => {
      expect(session()?.detailLoading).toBe(false)
    })

    expect(session()?.activeTurnToken).toBe(clientMessageId)
    expect(session()?.promptDeliveries[clientMessageId]).toMatchObject({
      clientMessageId,
      phase: "running",
      submittedAt: Date.parse(startedAt),
      acceptedAt: Date.parse(startedAt),
    })
  })

  it("does not resurrect a completed historical run as active", async () => {
    const clientMessageId = "optimistic-completed"
    mockGet.mockResolvedValue({
      ...detail([
        userTurn(clientMessageId, "done"),
        assistantTurn("answer", "finished"),
      ]),
      artifact_runs: [
        {
          id: "artifact-run-completed",
          conversation_id: CID,
          connection_id: "connection-1",
          client_message_id: clientMessageId,
          folder_id: 1,
          root_path: "/tmp/codeg",
          status: "completed",
          capture_incomplete: false,
          stop_reason: "end_turn",
          started_at: "2026-07-18T00:01:00.000Z",
          completed_at: "2026-07-18T00:02:00.000Z",
          changes: [],
        },
      ],
    })

    useConversationRuntimeStore.getState().actions.fetchDetail(CID)
    await vi.waitFor(() => {
      expect(session()?.detailLoading).toBe(false)
    })

    expect(session()?.activeTurnToken).toBeNull()
    expect(session()?.promptDeliveries).toEqual({})
  })
})

describe("syncViewerDetail — owner/guard no-ops", () => {
  it("no-ops when the conversation is not open (no runtime session)", () => {
    sync()
    expect(mockGet).not.toHaveBeenCalled()
  })

  it("no-ops for an owner mid-send (awaiting_persist)", () => {
    seed({
      syncState: "awaiting_persist",
      optimisticTurns: [userTurn("u", "hi")],
    })
    sync()
    expect(mockGet).not.toHaveBeenCalled()
  })

  it("no-ops for an owner still streaming (liveMessage present)", () => {
    seed({ liveMessage: liveMsg })
    sync()
    expect(mockGet).not.toHaveBeenCalled()
  })

  it("reads for a former owner while protecting an unflushed promoted reply", async () => {
    vi.useFakeTimers()
    const previous = detail(
      [userTurn("u0", "previous"), assistantTurn("a0", "previous reply")],
      20
    )
    const promoted = assistantTurn("a", "done")
    seed({ detail: previous, localTurns: [promoted], lastTurnOwned: true })
    mockGet.mockResolvedValue(previous)
    sync()
    await vi.advanceTimersByTimeAsync(10_000)
    expect(mockGet).toHaveBeenCalledTimes(5)
    expect(session()?.localTurns).toEqual([promoted])
    expect(session()?.detail).toEqual(previous)
  })

  it("recovers a later turn missed by a former owner and retires only a covered local batch", async () => {
    vi.useFakeTimers()
    const original = [userTurn("u1", "hi"), assistantTurn("a1", "hello")]
    const previous = detail(original, 20)
    seed({ detail: previous, localTurns: original, lastTurnOwned: true })
    const latest = detail(
      [
        ...original,
        userTurn("u2", "remote request"),
        assistantTurn("a2", "remote final"),
      ],
      60
    )
    mockGet.mockResolvedValueOnce(previous).mockResolvedValue(latest)

    sync()
    await vi.advanceTimersByTimeAsync(10_000)

    expect(mockGet).toHaveBeenCalledTimes(2)
    expect(session()?.localTurns).toEqual([])
    expect(session()?.detail?.turns.map((turn) => turn.id)).toEqual([
      "u1",
      "a1",
      "u2",
      "a2",
    ])
    for (const [, options] of mockGet.mock.calls) {
      expect(options).toMatchObject({
        userTurnLimit: HISTORY_PAGE_USER_TURNS,
        cacheMode: "reload",
      })
      expect(options).not.toHaveProperty("fromIndex")
    }
  })

  it("uses durable run identity when promoted and parsed turn ids differ", async () => {
    vi.useFakeTimers()
    seed({
      detail: detail([]),
      localTurns: [
        userTurn("client-prompt", "hi"),
        assistantTurn("live-reply", "hello"),
      ],
      lastTurnOwned: true,
    })
    const latest = {
      ...detail(
        [
          userTurn("parser-prompt", "hi"),
          assistantTurn("parser-reply", "hello"),
          userTurn("remote-prompt", "later"),
          assistantTurn("remote-reply", "latest"),
        ],
        60
      ),
      artifact_runs: [
        {
          id: "owner-run",
          conversation_id: CID,
          connection_id: "owner-connection",
          client_message_id: "client-prompt",
          folder_id: 1,
          root_path: "/tmp/test",
          status: "completed" as const,
          capture_incomplete: false,
          stop_reason: "end_turn",
          started_at: "2026-10-10T00:00:00Z",
          completed_at: "2026-10-10T00:00:01Z",
          changes: [],
        },
      ],
      deliverable_runs: [
        {
          turn_run_id: "owner-run",
          conversation_id: CID,
          client_message_id: "client-prompt",
          user_turn_id: "parser-prompt",
          started_at: "2026-10-10T00:00:00Z",
          completed_at: "2026-10-10T00:00:01Z",
          deliverables: [],
        },
      ],
    }
    mockGet.mockResolvedValue(latest)

    sync()
    await vi.advanceTimersByTimeAsync(10_000)

    expect(session()?.localTurns).toEqual([])
    expect(session()?.detail?.turns.slice(-1)[0]?.id).toBe("remote-reply")
    expect(mockGet).toHaveBeenCalledTimes(1)
  })

  it("keeps an unflushed owner final while adding a newer remote tail", async () => {
    vi.useFakeTimers()
    const local = assistantTurn(
      "local-final",
      "complete local answer still not on disk"
    )
    seed({
      detail: detail([assistantTurn("old", "old")], 20),
      localTurns: [local],
      lastTurnOwned: true,
    })
    const remote = detail(
      [
        userTurn("remote-user", "new request"),
        assistantTurn("remote-answer", "new reply"),
      ],
      60
    )
    mockGet.mockResolvedValue(remote)

    sync()
    await vi.advanceTimersByTimeAsync(10_000)

    expect(session()?.localTurns).toEqual([local])
    expect(session()?.detail?.turns).toEqual(remote.turns)
    expect(mockGet).toHaveBeenCalledTimes(5)
  })

  it("does not converge on a previous complete tail while the new transcript is still flushing", async () => {
    vi.useFakeTimers()
    const previous = detail(
      [userTurn("u1", "old prompt"), assistantTurn("a1", "old final")],
      20
    )
    seed({ detail: previous })
    const latest = detail(
      [
        ...previous.turns,
        userTurn("u2", "new prompt"),
        assistantTurn("a2", "new final"),
      ],
      60
    )
    mockGet
      .mockResolvedValueOnce(previous)
      .mockResolvedValueOnce(previous)
      .mockResolvedValue(latest)

    sync()
    await vi.advanceTimersByTimeAsync(10_000)

    expect(mockGet).toHaveBeenCalledTimes(3)
    expect(session()?.detail?.turns.slice(-1)[0]?.id).toBe("a2")
  })

  it("no-ops for a delegation-child dialog that adopted its reply (liveOwnsActiveTurn + localTurns)", async () => {
    vi.useFakeTimers()
    // A sub-agent dialog adopts the child's reply from the wire BEFORE the DB
    // catches up (`liveOwnsActiveTurn`), promoting it into `localTurns` with
    // lastTurnOwned=false (no owner send). A background transcript read that
    // still lacks (or only partially has) that reply must NOT clobber it — the
    // dialog owns its own promotion/dedup path.
    seed({
      liveOwnsActiveTurn: true,
      localTurns: [assistantTurn("a", "child reply")],
      lastTurnOwned: false,
    })
    mockGet.mockResolvedValue(detail([userTurn("u", "kickoff")], null, "u"))

    sync()
    await vi.advanceTimersByTimeAsync(10_000)
    expect(mockGet).not.toHaveBeenCalled()
  })

  it("DOES sync a marker-only delegation child (liveOwnsActiveTurn, no promoted reply)", async () => {
    vi.useFakeTimers()
    // The no-child-connection fallback opens the dialog and marks the session
    // live-owned, but nothing streams or promotes (localTurns empty, no
    // liveMessage). With nothing to protect, a completion nudge must still be
    // able to fold the child's finished reply from the transcript — otherwise the
    // dialog is stranded on its stale mount fetch until reopen.
    seed({
      liveOwnsActiveTurn: true,
      localTurns: [],
      detail: detail([userTurn("u", "kickoff")], null, "u"),
    })
    mockGet.mockResolvedValue(
      detail(
        [userTurn("u", "kickoff"), assistantTurn("a", "child reply")],
        null
      )
    )

    sync()
    await vi.advanceTimersByTimeAsync(0)

    expect(mockGet).toHaveBeenCalledTimes(1)
    expect((session()?.detail?.turns ?? []).map((t) => t.role)).toEqual([
      "user",
      "assistant",
    ])
  })

  it("DOES sync a viewer that streamed an earlier turn (localTurns, not owned)", async () => {
    vi.useFakeTimers()
    // A viewer promotes captured replies into `localTurns` too, but they are
    // already persisted (lastTurnOwned stays false). Such a viewer must still be
    // able to poll for a LATER short reply it missed — otherwise it strands after
    // its first captured turn, reproducing the original bug.
    seed({
      localTurns: [userTurn("u1", "hi"), assistantTurn("a1", "hello")],
      lastTurnOwned: false,
      detail: detail([userTurn("u1", "hi"), assistantTurn("a1", "hello")], 20),
    })
    mockGet.mockResolvedValue(
      detail(
        [
          userTurn("u1", "hi"),
          assistantTurn("a1", "hello"),
          userTurn("u2", "thanks"),
          assistantTurn("a2", "you're welcome"),
        ],
        60
      )
    )

    sync()
    await vi.advanceTimersByTimeAsync(0)

    expect(mockGet).toHaveBeenCalledTimes(1)
    expect((session()?.detail?.turns ?? []).map((t) => t.role)).toEqual([
      "user",
      "assistant",
      "user",
      "assistant",
    ])
  })
})

describe("reconcileCompletedTurn — lost stream recovery", () => {
  it("fills a background completion by task ID without dropping older loaded turns", async () => {
    vi.useFakeTimers()
    const old = assistantTurn("old", "loaded older history")
    const user = userTurn("u", "background request")
    const reply = {
      ...assistantTurn("a", "background final"),
      native_task_id: "background",
      usage: {
        input_tokens: 1,
        output_tokens: 1,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
      },
    }
    seed({ detail: detail([old, user], 10) })
    mockGet
      .mockResolvedValueOnce(detail([old, user, reply], 20))
      .mockResolvedValue(detail([user, { ...reply, duration_ms: 309047 }], 30))
    sync()
    await vi.advanceTimersByTimeAsync(10)
    expect(session()?.detail?.turns.slice(-1)[0]?.duration_ms).toBeUndefined()
    await vi.advanceTimersByTimeAsync(2000)
    expect(session()?.detail?.turns[0]).toBe(old)
    expect(session()?.detail?.turns.slice(-1)[0]?.duration_ms).toBe(309047)
    expect(mockGet).toHaveBeenCalledTimes(2)
  })

  it("restores exact native timings on a cold history load", async () => {
    const turns = [168795, 615361, 309047].map((duration_ms, i) => ({
      ...assistantTurn(`a-${i}`, `automation ${i}`),
      native_task_id: `task-${i}`,
      duration_ms,
    }))
    mockGet.mockResolvedValue(detail(turns))
    useConversationRuntimeStore.getState().actions.fetchDetail(CID)
    await vi.waitFor(() => expect(session()?.detailLoading).toBe(false))
    expect(
      session()?.detail?.turns.map((turn) => [
        turn.native_task_id,
        turn.duration_ms,
      ])
    ).toEqual([
      ["task-0", 168795],
      ["task-1", 615361],
      ["task-2", 309047],
    ])
    expect(mockGet).toHaveBeenCalledTimes(1)
  })

  it("fills timing when a cold refresh races the native terminal record", async () => {
    vi.useFakeTimers()
    const reply = {
      ...assistantTurn("a", "restored reply"),
      native_task_id: "refresh-task",
      usage: {
        input_tokens: 1,
        output_tokens: 1,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
      },
    }
    mockGet
      .mockResolvedValueOnce(detail([reply]))
      .mockResolvedValue(detail([{ ...reply, duration_ms: 168795 }]))
    useConversationRuntimeStore.getState().actions.fetchDetail(CID)
    await vi.advanceTimersByTimeAsync(10)
    expect(session()?.detail?.turns[0].duration_ms).toBeUndefined()
    await vi.advanceTimersByTimeAsync(2000)
    expect(session()?.detail?.turns[0].duration_ms).toBe(168795)
    expect(mockGet).toHaveBeenCalledTimes(2)
  })

  it("keeps reconciling native duration after the final reply and usage persist", async () => {
    vi.useFakeTimers()
    const user = userTurn("u", "ordinary request")
    const reply = {
      ...assistantTurn("a", "complete final"),
      native_task_id: "native-ordinary",
      usage: {
        input_tokens: 10,
        output_tokens: 5,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
      },
    }
    const first = detail([user, reply])
    const timed = detail([user, { ...reply, duration_ms: 168795 }])
    seed({
      detail: detail([user]),
      localTurns: [assistantTurn("live-a", "complete final")],
      historyAssistantBaseline: 0,
      lastTurnOwned: true,
    })
    mockGet.mockResolvedValueOnce(first).mockResolvedValue(timed)
    useConversationRuntimeStore
      .getState()
      .actions.reconcileCompletedTurn(CID, CID, "u")
    await vi.advanceTimersByTimeAsync(100)
    expect(session()?.detail?.turns.slice(-1)[0]?.duration_ms).toBeUndefined()
    await vi.advanceTimersByTimeAsync(40_000)
    expect(session()?.detail?.turns.slice(-1)[0]?.duration_ms).toBe(168795)
    expect(session()?.detail?.turns.slice(-1)[0]?.native_task_id).toBe(
      "native-ordinary"
    )
    expect(mockGet).toHaveBeenCalledTimes(2)
  })

  it("does not replace a promoted final with a partial bounded page", async () => {
    vi.useFakeTimers()
    const user = userTurn("u", "request")
    const reply = assistantTurn("a", "complete local final")
    const partial = {
      ...detail([user, assistantTurn("partial-a", "partial answer")]),
      history_page: { has_more: true, loaded_turns: 2, page_complete: false },
    }
    seed({
      detail: detail([user]),
      localTurns: [reply],
      lastTurnOwned: true,
      historyAssistantBaseline: 0,
    })
    mockGet.mockResolvedValue(partial)
    const reconciled = vi.fn()

    useConversationRuntimeStore
      .getState()
      .actions.reconcileCompletedTurn(CID, CID, "u", reconciled)
    await vi.advanceTimersByTimeAsync(10_000)

    expect(session()?.localTurns).toEqual([reply])
    expect(reconciled).not.toHaveBeenCalled()
    expect(mockGet).toHaveBeenCalledTimes(6)
  })

  it("polls past a trailing user record and folds the persisted final reply", async () => {
    vi.useFakeTimers()
    const stale = detail([userTurn("u", "image + text")])
    const settled = detail([
      userTurn("u", "image + text"),
      assistantTurn("a", "final reply"),
    ])
    seed({
      detail: stale,
      localTurns: [userTurn("optimistic-image", "image + text")],
      historyAssistantBaseline: 0,
      lastTurnOwned: true,
    })
    mockGet.mockResolvedValueOnce(stale).mockResolvedValue(settled)
    const reconciled = vi.fn()

    useConversationRuntimeStore
      .getState()
      .actions.reconcileCompletedTurn(CID, CID, "optimistic-image", reconciled)
    await vi.advanceTimersByTimeAsync(10_000)

    expect(mockGet).toHaveBeenCalledTimes(2)
    expect(session()?.localTurns).toEqual([])
    const turns = session()?.detail?.turns ?? []
    expect(turns[turns.length - 1]?.role).toBe("assistant")
    expect(reconciled).toHaveBeenCalledTimes(1)
  })

  it("retires a promoted reply when a publish refresh already counted the persisted assistant", async () => {
    vi.useFakeTimers()
    const clientMessageId = "optimistic-published"
    const persisted = {
      ...detail([
        userTurn("parser-user", "generate a PDF"),
        assistantTurn("parser-assistant", "PDF created"),
      ]),
      artifact_runs: [
        {
          id: "run-published",
          conversation_id: CID,
          connection_id: "connection-1",
          client_message_id: clientMessageId,
          folder_id: 1,
          root_path: "/tmp/codeg",
          status: "completed" as const,
          capture_incomplete: false,
          stop_reason: "end_turn",
          started_at: "2026-08-05T03:01:20.000Z",
          completed_at: "2026-08-05T03:13:10.000Z",
          changes: [],
        },
      ],
      deliverable_runs: [
        {
          turn_run_id: "run-published",
          conversation_id: CID,
          client_message_id: clientMessageId,
          user_turn_id: "parser-user",
          started_at: "2026-08-05T03:01:20.000Z",
          completed_at: "2026-08-05T03:13:10.000Z",
          // The reconciliation proof uses the durable run/user link; card
          // payload rendering is covered by message-list-view tests.
          deliverables: [],
        },
      ],
    }
    const promotedReply = assistantTurn("live-assistant", "PDF created")
    seed({
      detail: persisted,
      localTurns: [userTurn(clientMessageId, "generate a PDF"), promotedReply],
      // A publish event refreshed detail after the parser had already created
      // its assistant row, so the old count-growth test can never succeed.
      historyAssistantBaseline: 1,
      lastTurnOwned: true,
    })
    mockGet.mockResolvedValue(persisted)

    useConversationRuntimeStore
      .getState()
      .actions.reconcileCompletedTurn(CID, CID, clientMessageId)
    await vi.advanceTimersByTimeAsync(100)

    expect(mockGet).toHaveBeenCalledTimes(1)
    expect(session()?.localTurns).toEqual([])
    expect(session()?.detail).toBe(persisted)
  })

  it("never replaces a promoted reply with an incompletely flushed transcript", async () => {
    vi.useFakeTimers()
    const stale = detail([userTurn("u", "prompt")])
    const promotedReply = assistantTurn("live-a", "complete stream reply")
    seed({
      detail: stale,
      localTurns: [userTurn("optimistic-1", "prompt"), promotedReply],
      historyAssistantBaseline: 0,
      lastTurnOwned: true,
    })
    mockGet.mockResolvedValue(stale)

    useConversationRuntimeStore
      .getState()
      .actions.reconcileCompletedTurn(CID, CID, "optimistic-1")
    await vi.advanceTimersByTimeAsync(20_000)

    expect(session()?.localTurns).toContainEqual(promotedReply)
    expect(session()?.detail).toBe(stale)
  })

  it("does not erase an unaccepted optimistic prompt by mistaking the previous reply for its final", async () => {
    vi.useFakeTimers()
    const oldDetail = detail([
      userTurn("old-u", "old prompt"),
      assistantTurn("old-a", "old reply"),
    ])
    const failedPrompt = userTurn("optimistic-failed", "image prompt")
    seed({
      detail: oldDetail,
      optimisticTurns: [failedPrompt],
      activeTurnToken: failedPrompt.id,
      syncState: "idle",
      historyAssistantBaseline: 1,
    })
    mockGet.mockResolvedValue(oldDetail)

    useConversationRuntimeStore
      .getState()
      .actions.reconcileCompletedTurn(CID, CID, failedPrompt.id)
    await vi.advanceTimersByTimeAsync(20_000)

    expect(session()?.optimisticTurns).toContainEqual(failedPrompt)
    expect(session()?.detail).toBe(oldDetail)
  })

  it("stops an older reconciliation when a queued follow-up starts", async () => {
    vi.useFakeTimers()
    seed({
      detail: detail([userTurn("u", "first")]),
      localTurns: [assistantTurn("a", "first reply")],
      syncState: "awaiting_persist",
    })

    useConversationRuntimeStore
      .getState()
      .actions.reconcileCompletedTurn(CID, CID, "optimistic-first")
    await vi.advanceTimersByTimeAsync(100)

    expect(mockGet).not.toHaveBeenCalled()
  })
})

describe("syncViewerDetail — pure viewer refetch", () => {
  it("lands a completed reply on the first fetch and does not schedule a retry", async () => {
    vi.useFakeTimers()
    // Pure viewer showing only the synthesized user prompt (optimistic, NOT
    // awaiting_persist — the viewer didn't send).
    seed({
      optimisticTurns: [userTurn("u", "hi")],
      detail: detail([userTurn("u", "hi")], 10),
    })
    mockGet.mockResolvedValue(
      detail([userTurn("u", "hi"), assistantTurn("a", "Hi! …")], 42)
    )

    sync()
    await vi.advanceTimersByTimeAsync(0)

    expect(mockGet).toHaveBeenCalledTimes(1)
    expect(mockGet).toHaveBeenCalledWith(
      CID,
      expect.objectContaining({ userTurnLimit: HISTORY_PAGE_USER_TURNS })
    )
    const turns = session()?.detail?.turns ?? []
    expect(turns.map((t) => t.role)).toEqual(["user", "assistant"])
    // The persisted load replaces the synthesized optimistic prompt.
    expect(session()?.optimisticTurns).toEqual([])

    // A trailing assistant reply means "nothing left to wait for" — no retry.
    await vi.advanceTimersByTimeAsync(5000)
    expect(mockGet).toHaveBeenCalledTimes(1)
  })

  it("polls while the transcript still ends at the user prompt, then stops once the reply flushes", async () => {
    vi.useFakeTimers()
    seed({
      optimisticTurns: [userTurn("u", "hi")],
      detail: detail([userTurn("u", "hi")], 10),
    })
    // Attempt 0: reply not flushed yet (transcript still ends at the prompt).
    // Attempt 1: reply landed.
    mockGet
      .mockResolvedValueOnce(detail([userTurn("u", "hi")], 10))
      .mockResolvedValueOnce(
        detail([userTurn("u", "hi"), assistantTurn("a", "Hi! …")], 42)
      )

    sync()
    await vi.advanceTimersByTimeAsync(0)
    expect(mockGet).toHaveBeenCalledTimes(1)
    // Still only the prompt — reply not surfaced yet.
    expect((session()?.detail?.turns ?? []).map((t) => t.role)).toEqual([
      "user",
    ])

    // Second attempt fires after the first backoff delay.
    await vi.advanceTimersByTimeAsync(300)
    expect(mockGet).toHaveBeenCalledTimes(2)
    expect((session()?.detail?.turns ?? []).map((t) => t.role)).toEqual([
      "user",
      "assistant",
    ])

    // Converged — no further polling.
    await vi.advanceTimersByTimeAsync(5000)
    expect(mockGet).toHaveBeenCalledTimes(2)
  })

  it("keeps polling while the turn is in flight even if the tail is a partial assistant turn", async () => {
    vi.useFakeTimers()
    seed({
      optimisticTurns: [userTurn("u", "hi")],
      detail: detail([userTurn("u", "hi")], 10),
    })
    // OpenCode/Gemini shape: a PARTIAL assistant turn is persisted mid-stream,
    // so the tail is already `assistant`, but the backend still reports the turn
    // as in flight — a role-only check would stop early and miss the final reply.
    mockGet
      .mockResolvedValueOnce(
        detail([userTurn("u", "hi"), assistantTurn("a", "Hi")], 20, "u")
      )
      .mockResolvedValueOnce(
        detail([userTurn("u", "hi"), assistantTurn("a", "Hi! …")], 42, null)
      )

    sync()
    await vi.advanceTimersByTimeAsync(0)
    expect(mockGet).toHaveBeenCalledTimes(1)

    // The in-flight flag kept the poll going despite the assistant tail.
    await vi.advanceTimersByTimeAsync(300)
    expect(mockGet).toHaveBeenCalledTimes(2)
    expect((session()?.detail?.turns ?? []).map((t) => t.role)).toEqual([
      "user",
      "assistant",
    ])

    await vi.advanceTimersByTimeAsync(5000)
    expect(mockGet).toHaveBeenCalledTimes(2)
  })

  it("gives up after a bounded number of attempts when the reply never lands", async () => {
    vi.useFakeTimers()
    seed({ detail: detail([userTurn("u", "hi")], 10) })
    // Every read still ends at the user prompt (e.g. a cancelled turn).
    mockGet.mockResolvedValue(detail([userTurn("u", "hi")], 10))

    sync()
    await vi.advanceTimersByTimeAsync(0)
    await vi.advanceTimersByTimeAsync(10_000)
    // 5 delays configured → at most 5 attempts, then it stops for good.
    expect(mockGet).toHaveBeenCalledTimes(5)
  })

  it("does not commit its read when a concurrent panel fetch superseded the generation", async () => {
    vi.useFakeTimers()
    seed({ detail: detail([userTurn("u", "hi")], 10) })
    mockGet
      // attempt 0: a reply-pending read that will be superseded before it lands.
      .mockResolvedValueOnce(detail([userTurn("u", "hi")], 20))
      // the concurrent panel refetch — never resolves, just owns the generation.
      .mockImplementation(() => new Promise<DbConversationDetail>(() => {}))

    sync()
    // A panel refetch bumps the shared fetch generation before attempt 0 lands.
    useConversationRuntimeStore.getState().actions.refetchDetail(CID)
    await vi.advanceTimersByTimeAsync(0)

    // attempt 0 resolved but was superseded → it must NOT clobber the detail the
    // panel refetch now owns (watermark stays at the seeded 10, not 20).
    expect(session()?.detail?.transcript_watermark).toBe(10)
  })

  it("fetches by dbConversationId when the runtime key is a virtual id", async () => {
    vi.useFakeTimers()
    useConversationRuntimeStore.setState({
      byConversationId: new Map([
        [
          -7,
          {
            ...emptySession(-7),
            dbConversationId: 500,
            detail: detail([userTurn("u", "hi")], 10),
            optimisticTurns: [userTurn("u", "hi")],
          },
        ],
      ]),
    })
    mockGet.mockResolvedValue(
      detail([userTurn("u", "hi"), assistantTurn("a", "Hi! …")], 42)
    )

    useConversationRuntimeStore.getState().actions.syncViewerDetail(-7)
    await vi.advanceTimersByTimeAsync(0)
    expect(mockGet).toHaveBeenCalledWith(
      500,
      expect.objectContaining({ userTurnLimit: HISTORY_PAGE_USER_TURNS })
    )
  })

  it("routes a positive-id nudge to a draft tab keyed by a negative runtime id", async () => {
    vi.useFakeTimers()
    // A draft-originated tab keeps its virtual (negative) runtime key while
    // storing the positive DB id in `dbConversationId`. The `conversation://
    // changed` nudge carries the positive id, so the action must reverse-resolve
    // it to the negative-keyed session (a direct lookup would miss it entirely).
    useConversationRuntimeStore.setState({
      byConversationId: new Map([
        [
          -7,
          {
            ...emptySession(-7),
            dbConversationId: CID,
            detail: detail([userTurn("u", "hi")], 10),
            optimisticTurns: [userTurn("u", "hi")],
          },
        ],
      ]),
    })
    mockGet.mockResolvedValue(
      detail([userTurn("u", "hi"), assistantTurn("a", "Hi! …")], 42)
    )

    // Nudge with the POSITIVE db id, exactly as the workspace side-channel does.
    useConversationRuntimeStore.getState().actions.syncViewerDetail(CID)
    await vi.advanceTimersByTimeAsync(0)

    expect(mockGet).toHaveBeenCalledWith(
      CID,
      expect.objectContaining({ userTurnLimit: HISTORY_PAGE_USER_TURNS })
    )
    const turns =
      useConversationRuntimeStore.getState().byConversationId.get(-7)?.detail
        ?.turns ?? []
    expect(turns.map((t) => t.role)).toEqual(["user", "assistant"])
  })

  it("commits the final reply of a no-watermark agent that grew a partial in place", async () => {
    vi.useFakeTimers()
    // OpenCode/Gemini have no transcript watermark and rewrite the SAME assistant
    // turn in place as it grows. The partial and the final read therefore share a
    // null watermark AND turn count, so the byte/count `changed` guard can't tell
    // them apart — only the settle transition (`in_flight` clearing) can. Without
    // a settle-commit the viewer would freeze on the partial.
    seed({
      optimisticTurns: [userTurn("u", "hi")],
      detail: detail([userTurn("u", "hi")], null),
    })
    mockGet
      // Attempt 0: a partial assistant turn, still in flight → commits (count
      // grew from 1 to 2) and keeps polling.
      .mockResolvedValueOnce(
        detail([userTurn("u", "hi"), assistantTurn("a", "Hi")], null, "u")
      )
      // Attempt 1: the FINAL content, settled (in_flight cleared). Same null
      // watermark AND same turn count as attempt 0 → `changed` is false; only the
      // settle-commit lands it.
      .mockResolvedValueOnce(
        detail(
          [
            userTurn("u", "hi"),
            assistantTurn("a", "Hi there, how can I help?"),
          ],
          null,
          null
        )
      )

    sync()
    await vi.advanceTimersByTimeAsync(0)
    expect(mockGet).toHaveBeenCalledTimes(1)

    await vi.advanceTimersByTimeAsync(300)
    expect(mockGet).toHaveBeenCalledTimes(2)
    const turns = session()?.detail?.turns ?? []
    const lastText = turns[turns.length - 1]?.blocks?.[0]
    expect(lastText?.type === "text" ? lastText.text : null).toBe(
      "Hi there, how can I help?"
    )

    // Converged after the settle-commit — no further polling.
    await vi.advanceTimersByTimeAsync(5000)
    expect(mockGet).toHaveBeenCalledTimes(2)
  })
})

// A view's own detail load (a tab's first fetch, a reload) sets
// `detailLoading`, and only its own result clears it. A poll read issued under
// it bumped the fetch generation, so that result was dropped as stale: if the
// poll then never committed (every read failing), the view was left loading
// for good. The overlap is common: the load's own read can backfill the title,
// and the upsert that broadcasts is a nudge.
describe("syncViewerDetail — a detail load in flight", () => {
  it("lets the load land, then polls on from there", async () => {
    vi.useFakeTimers()
    seed({})
    let landLoad: (d: DbConversationDetail) => void = () => {}
    mockGet
      .mockImplementationOnce(
        () =>
          new Promise<DbConversationDetail>((r) => {
            landLoad = r
          })
      )
      .mockResolvedValue(
        detail([userTurn("u", "hi"), assistantTurn("a", "Hi! …")], 42)
      )
    useConversationRuntimeStore.getState().actions.fetchDetail(CID)

    sync()
    await vi.advanceTimersByTimeAsync(0)
    // The poll holds its read while the load is in flight…
    expect(mockGet).toHaveBeenCalledTimes(1)

    // …the load lands, from before the reply reached disk…
    landLoad(detail([userTurn("u", "hi")], 10))
    await vi.advanceTimersByTimeAsync(0)
    expect(session()?.detailLoading).toBe(false)
    expect(session()?.detail?.transcript_watermark).toBe(10)

    // …and the poll goes on to pick the reply up.
    await vi.advanceTimersByTimeAsync(300)
    expect(mockGet).toHaveBeenCalledTimes(2)
    expect((session()?.detail?.turns ?? []).map((t) => t.role)).toEqual([
      "user",
      "assistant",
    ])
  })

  it("never leaves the view loading when every poll read fails", async () => {
    vi.useFakeTimers()
    seed({})
    let landLoad: (d: DbConversationDetail) => void = () => {}
    mockGet
      .mockImplementationOnce(
        () =>
          new Promise<DbConversationDetail>((r) => {
            landLoad = r
          })
      )
      .mockRejectedValue(new Error("server down"))
    useConversationRuntimeStore.getState().actions.fetchDetail(CID)

    sync()
    await vi.advanceTimersByTimeAsync(0)
    landLoad(detail([userTurn("u", "hi"), assistantTurn("a", "Hi! …")], 42))
    await vi.advanceTimersByTimeAsync(10_000)

    expect(session()?.detailLoading).toBe(false)
    expect(session()?.detail?.transcript_watermark).toBe(42)
  })

  it("stops waiting behind a load that never settles", async () => {
    vi.useFakeTimers()
    seed({})
    mockGet.mockImplementation(
      () => new Promise<DbConversationDetail>(() => {})
    )
    useConversationRuntimeStore.getState().actions.fetchDetail(CID)

    sync()
    await vi.advanceTimersByTimeAsync(60_000)

    expect(mockGet).toHaveBeenCalledTimes(1)
    expect(vi.getTimerCount()).toBe(0)
  })
})

describe("syncViewerDetail — cancellation", () => {
  it("removeConversation cancels a pending poll (no further fetch)", async () => {
    vi.useFakeTimers()
    seed({ detail: detail([userTurn("u", "hi")], 10) })
    mockGet.mockResolvedValue(detail([userTurn("u", "hi")], 10)) // stays pending

    sync()
    await vi.advanceTimersByTimeAsync(0)
    expect(mockGet).toHaveBeenCalledTimes(1)

    useConversationRuntimeStore.getState().actions.removeConversation(CID)
    await vi.advanceTimersByTimeAsync(10_000)
    // The scheduled retry was cancelled with the tab.
    expect(mockGet).toHaveBeenCalledTimes(1)
  })

  it("stops polling if the viewer starts its own turn mid-poll", async () => {
    vi.useFakeTimers()
    seed({ detail: detail([userTurn("u", "hi")], 10) })
    mockGet.mockResolvedValue(detail([userTurn("u", "hi")], 10))

    sync()
    await vi.advanceTimersByTimeAsync(0)
    expect(mockGet).toHaveBeenCalledTimes(1)

    // The viewer becomes an owner (local send → awaiting_persist). The next
    // tick's pure-viewer guard must abort the poll.
    seed({
      detail: detail([userTurn("u", "hi")], 10),
      syncState: "awaiting_persist",
      optimisticTurns: [userTurn("u2", "again")],
    })
    await vi.advanceTimersByTimeAsync(10_000)
    expect(mockGet).toHaveBeenCalledTimes(1)
  })
})
