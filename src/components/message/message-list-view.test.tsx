import { describe, expect, it } from "vitest"

import {
  associateDeliverablesWithUserTurns,
  mergeConsecutiveAssistantTurns,
  replyDeliverablesForRun,
  resolveDeliverableAssociations,
  type MergedAssistantRunCache,
  type ResolvedMessageGroup,
  type ThreadRenderItem,
} from "./message-list-view"
import type {
  ConversationDeliverable,
  ConversationTurnDeliverableSet,
} from "@/lib/types"

type ThreadItem = Parameters<typeof mergeConsecutiveAssistantTurns>[0][number]
type TurnItem = Extract<ThreadItem, { kind: "turn" }>

function assistantItem(
  id: string,
  groupOverrides: Partial<TurnItem["group"]> = {}
): ThreadItem {
  return {
    key: `persisted-${id}`,
    kind: "turn",
    group: {
      id,
      role: "assistant",
      parts: [{ type: "text", text: `reply ${id}` }],
      resources: [],
      images: [],
      ...groupOverrides,
    },
    phase: "persisted",
    showStats: false,
    isRoleTransition: false,
    previousUserIndex: null,
  }
}

describe("mergeConsecutiveAssistantTurns", () => {
  it("surfaces completion time patched onto a non-last sub-turn", () => {
    // Real-device bug (Cursor session 118b6805): the post-turn metadata
    // patch head-aligns onto the FIRST local sub-turn when the parser emits
    // fewer turns than the live stream split into. The merged footer must
    // still show that completion time (and its duration), not the last
    // sub-turn's empty fields.
    const merged = mergeConsecutiveAssistantTurns([
      assistantItem("a", {
        duration_ms: 15_975,
        completed_at: "2026-07-19T05:25:22.851Z",
      }),
      assistantItem("b"),
    ])
    expect(merged).toHaveLength(1)
    const item = merged[0] as TurnItem
    expect(item.group.completed_at).toBe("2026-07-19T05:25:22.851Z")
    expect(item.group.duration_ms).toBe(15_975)
  })

  it("keeps the latest completion when several sub-turns carry one", () => {
    const merged = mergeConsecutiveAssistantTurns([
      assistantItem("a", { completed_at: "2026-07-19T05:25:10.000Z" }),
      assistantItem("b", { completed_at: "2026-07-19T05:25:22.851Z" }),
    ])
    expect(merged).toHaveLength(1)
    const item = merged[0] as TurnItem
    expect(item.group.completed_at).toBe("2026-07-19T05:25:22.851Z")
  })

  it("does not fold a compaction divider into the preceding assistant reply", () => {
    // The compaction event sits BETWEEN two assistant replies (the reply before
    // `/compact` and the next). Two bare assistant turns would merge into one;
    // the dedicated "compaction" item must break that run so the divider renders
    // standalone in the correct between-turns position (and the first reply keeps
    // its own footer).
    const compaction: ThreadItem = {
      key: "persisted-compact",
      kind: "compaction",
      meta: { contextCompaction: true, tokensBefore: 51777, tokensAfter: 4616 },
    }
    // Sanity: without the divider, the two assistant turns DO merge to one.
    expect(
      mergeConsecutiveAssistantTurns([assistantItem("a"), assistantItem("b")])
    ).toHaveLength(1)
    // With the divider between them, the run is broken → 3 standalone items.
    const merged = mergeConsecutiveAssistantTurns([
      assistantItem("a"),
      compaction,
      assistantItem("b"),
    ])
    expect(merged.map((it) => it.kind)).toEqual(["turn", "compaction", "turn"])
  })
})

describe("associateDeliverablesWithUserTurns", () => {
  const deliverable = (
    id: string,
    overrides: Partial<ConversationDeliverable> = {}
  ) =>
    ({
      id,
      role: "primary",
      category: "standalone_output",
      source: "declared",
      is_valid: true,
      ...overrides,
    }) as ConversationDeliverable
  const run = (
    id: string,
    clientMessageId: string | null,
    startedAt: string,
    completedAt: string,
    outputId: string
  ): ConversationTurnDeliverableSet => ({
    turn_run_id: id,
    conversation_id: 1,
    client_message_id: clientMessageId,
    started_at: startedAt,
    completed_at: completedAt,
    deliverables: [deliverable(outputId)],
  })

  it("uses the exact live client message id when it still exists", () => {
    const mapped = associateDeliverablesWithUserTurns(
      [
        run(
          "run-1",
          "optimistic-1",
          "2026-07-20T10:00:00Z",
          "2026-07-20T10:01:00Z",
          "output-1"
        ),
      ],
      [{ id: "optimistic-1", timestamp: "2026-07-20T10:00:01Z" }]
    )
    expect(mapped.get("optimistic-1")?.[0].id).toBe("output-1")
  })

  it("prefers the backend durable user turn id on a different machine", () => {
    const linked = run(
      "run-1",
      "optimistic-only-on-sender",
      "2026-07-20T10:00:00Z",
      "2026-07-20T10:01:00Z",
      "output-1"
    )
    linked.user_turn_id = "parsed-user-turn"
    const mapped = associateDeliverablesWithUserTurns(
      [linked],
      [{ id: "parsed-user-turn", timestamp: "2026-07-20T12:00:00Z" }]
    )
    expect(mapped.get("parsed-user-turn")?.[0].id).toBe("output-1")
  })

  it("recovers the producing reply by timestamp after a cold parser reload", () => {
    const mapped = associateDeliverablesWithUserTurns(
      [
        run(
          "run-1",
          "optimistic-gone",
          "2026-07-20T10:00:00Z",
          "2026-07-20T10:05:00Z",
          "output-1"
        ),
      ],
      [
        { id: "old-turn", timestamp: "2026-07-20T09:00:00Z" },
        { id: "parsed-user-turn", timestamp: "2026-07-20T10:00:01Z" },
        // A steer recorded later in the same run must not steal the card from
        // the initial user prompt.
        { id: "parsed-steer", timestamp: "2026-07-20T10:03:00Z" },
      ]
    )
    expect(mapped.get("parsed-user-turn")?.[0].id).toBe("output-1")
    expect(mapped.has("old-turn")).toBe(false)
    expect(mapped.has("parsed-steer")).toBe(false)
  })

  it("does not guess when every user turn is outside the run window", () => {
    const mapped = associateDeliverablesWithUserTurns(
      [
        run(
          "run-1",
          "optimistic-gone",
          "2026-07-20T10:00:00Z",
          "2026-07-20T10:01:00Z",
          "output-1"
        ),
      ],
      [{ id: "unrelated", timestamp: "2026-07-20T12:00:00Z" }]
    )
    expect(mapped.size).toBe(0)
  })
})

describe("replyDeliverablesForRun", () => {
  const output = (
    id: string,
    overrides: Partial<ConversationDeliverable> = {}
  ) =>
    ({
      id,
      role: "primary",
      category: "standalone_output",
      source: "declared",
      is_valid: true,
      change_kind: "created",
      ...overrides,
    }) as ConversationDeliverable

  it("treats every explicit declaration as the authoritative turn set", () => {
    const all = [
      output("report"),
      output("supporting", { role: "supporting" }),
      output("source", { category: "code_change" }),
      output("missing-inferred", { source: "inferred", is_valid: false }),
      output("expected-inferred", { source: "inferred" }),
    ]
    expect(replyDeliverablesForRun(all).map((item) => item.id)).toEqual([
      "report",
      "supporting",
      "source",
    ])
    expect(all).toHaveLength(5)
  })

  it("shows supporting declarations and ignores inferred QA noise", () => {
    const all = [
      output("designed-pdf", { role: "supporting" }),
      output("source", {
        role: "supporting",
        category: "code_change",
      }),
      output("ambiguous-image", {
        role: "supporting",
        source: "inferred",
      }),
    ]

    expect(replyDeliverablesForRun(all).map((item) => item.id)).toEqual([
      "designed-pdf",
      "source",
    ])
  })

  it("does not hide a turn that contains multiple declared primary files", () => {
    const all = [
      output("final-pdf"),
      output("second-primary-pdf"),
      output("qa-page-1", { source: "inferred" }),
      output("merged-docx", { role: "supporting" }),
    ]

    expect(replyDeliverablesForRun(all).map((item) => item.id)).toEqual([
      "final-pdf",
      "second-primary-pdf",
      "merged-docx",
    ])
  })

  it("shows server-filtered inferred outputs and omits empty cards", () => {
    expect(replyDeliverablesForRun([])).toEqual([])
    expect(
      replyDeliverablesForRun([
        output("qa-supporting", {
          role: "supporting",
          source: "inferred",
        }),
      ])
    ).toHaveLength(1)
    expect(
      replyDeliverablesForRun([
        output("invalid", {
          source: "inferred",
          is_valid: false,
        }),
      ])
    ).toEqual([])
  })
})

describe("resolveDeliverableAssociations", () => {
  const declared = {
    id: "published-pdf",
    role: "primary",
    category: "standalone_output",
    source: "declared",
    is_valid: true,
    change_kind: "created",
  } as ConversationDeliverable

  it("retains a persisted declaration when historical turn linkage is missing", () => {
    const result = resolveDeliverableAssociations(
      [
        {
          turn_run_id: "run-orphaned",
          conversation_id: 1,
          client_message_id: null,
          user_turn_id: null,
          started_at: "invalid",
          completed_at: "invalid",
          deliverables: [declared],
        },
      ],
      [{ id: "parsed-user", timestamp: "2026-08-05T03:01:20Z" }]
    )

    expect(result.byUserId.size).toBe(0)
    expect(result.unassociated.map((item) => item.id)).toEqual([
      "published-pdf",
    ])
  })

  it("does not manufacture an unassociated card for a turn without outputs", () => {
    const result = resolveDeliverableAssociations(
      [
        {
          turn_run_id: "run-empty",
          conversation_id: 1,
          client_message_id: null,
          user_turn_id: null,
          started_at: "2026-08-05T03:01:20Z",
          completed_at: "2026-08-05T03:02:20Z",
          deliverables: [],
        },
      ],
      [{ id: "parsed-user", timestamp: "2026-08-05T03:01:20Z" }]
    )

    expect(result.byUserId.size).toBe(0)
    expect(result.unassociated).toEqual([])
  })
})

function makeGroup(
  role: "user" | "assistant",
  id: string
): ResolvedMessageGroup {
  return { id, role, parts: [], resources: [], images: [] }
}

// Fresh render-item objects per call, like the rawItems map in threadItems —
// only `group` and `key` carry identity.
function makeItem(
  group: ResolvedMessageGroup,
  index: number,
  phase: "persisted" | "optimistic" | "streaming" = "persisted"
): ThreadRenderItem {
  return {
    key: `${phase}-${group.id}-${index}`,
    kind: "turn",
    group,
    phase,
    showStats: false,
    isRoleTransition: false,
    previousUserIndex: null,
  }
}

function makeUserItem(id: string, index: number): ThreadRenderItem {
  const item = makeItem(makeGroup("user", id), index)
  if (item.kind === "turn") {
    item.group.parts = [{ type: "text", text: "hi" }]
  }
  return item
}

describe("mergeConsecutiveAssistantTurns merged-run cache", () => {
  it("reuses the merged item and group when membership is unchanged", () => {
    const cache: MergedAssistantRunCache = new WeakMap()
    const g1 = makeGroup("assistant", "a1")
    const g2 = makeGroup("assistant", "a2")

    const out1 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), makeItem(g2, 1)],
      cache
    )
    const out2 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), makeItem(g2, 1)],
      cache
    )

    expect(out1).toHaveLength(1)
    const first = out1[0]
    const second = out2[0]
    if (first.kind !== "turn" || second.kind !== "turn") {
      throw new Error("expected turn items")
    }
    expect(second).toBe(first)
    expect(second.group).toBe(first.group)
    expect(second.group.parts).toBe(first.group.parts)
    expect(first.key).toBe("merged-persisted-a1-0")
    expect(first.group.id).toBe("a1")
  })

  it("rebuilds a run whose member changed without touching a neighboring run", () => {
    const cache: MergedAssistantRunCache = new WeakMap()
    const g1 = makeGroup("assistant", "a1")
    const g2 = makeGroup("assistant", "a2")
    const g3 = makeGroup("assistant", "a3")
    const g4 = makeGroup("assistant", "a4")

    const out1 = mergeConsecutiveAssistantTurns(
      [
        makeItem(g1, 0),
        makeItem(g2, 1),
        makeUserItem("u1", 2),
        makeItem(g3, 3),
        makeItem(g4, 4),
      ],
      cache
    )
    // Second member of run A re-adapted (new group object, e.g. its turn was
    // reloaded); run B untouched.
    const g2b = makeGroup("assistant", "a2")
    const out2 = mergeConsecutiveAssistantTurns(
      [
        makeItem(g1, 0),
        makeItem(g2b, 1),
        makeUserItem("u1", 2),
        makeItem(g3, 3),
        makeItem(g4, 4),
      ],
      cache
    )

    expect(out2[0]).not.toBe(out1[0])
    expect(out2[2]).toBe(out1[2])
  })

  it("misses when the run gains a member, then caches the new membership", () => {
    const cache: MergedAssistantRunCache = new WeakMap()
    const g1 = makeGroup("assistant", "a1")
    const g2 = makeGroup("assistant", "a2")
    const g3 = makeGroup("assistant", "a3")

    const out1 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), makeItem(g2, 1)],
      cache
    )
    const out2 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), makeItem(g2, 1), makeItem(g3, 2)],
      cache
    )
    const out3 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), makeItem(g2, 1), makeItem(g3, 2)],
      cache
    )

    expect(out2[0]).not.toBe(out1[0])
    expect(out3[0]).toBe(out2[0])
  })

  it("keeps cache hits across interleaved empty (skipped) turn items", () => {
    const cache: MergedAssistantRunCache = new WeakMap()
    const g1 = makeGroup("assistant", "a1")
    const g2 = makeGroup("assistant", "a2")
    const emptyUser = () => makeItem(makeGroup("user", "empty"), 1)

    const out1 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), emptyUser(), makeItem(g2, 2)],
      cache
    )
    const out2 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), emptyUser(), makeItem(g2, 2)],
      cache
    )

    // The empty user turn is transparent: one merged item, no user item.
    expect(out1).toHaveLength(1)
    expect(out2[0]).toBe(out1[0])
  })

  it("passes single-turn runs through untouched without caching", () => {
    const cache: MergedAssistantRunCache = new WeakMap()
    const item = makeItem(makeGroup("assistant", "solo"), 0)

    const out = mergeConsecutiveAssistantTurns([item], cache)

    expect(out).toHaveLength(1)
    expect(out[0]).toBe(item)
  })

  it("still merges correctly without a cache", () => {
    const g1 = makeGroup("assistant", "a1")
    const g2 = makeGroup("assistant", "a2")

    const out1 = mergeConsecutiveAssistantTurns([
      makeItem(g1, 0),
      makeItem(g2, 1),
    ])
    const out2 = mergeConsecutiveAssistantTurns([
      makeItem(g1, 0),
      makeItem(g2, 1),
    ])

    expect(out1).toHaveLength(1)
    expect(out2[0]).not.toBe(out1[0])
    expect(out2[0]).toEqual(out1[0])
  })
})
