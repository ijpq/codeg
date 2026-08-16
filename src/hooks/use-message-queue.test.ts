import { describe, it, expect, beforeEach, afterEach } from "vitest"
import { act, renderHook } from "@testing-library/react"
import {
  isQueuedGuideTargetCurrent,
  useMessageQueue,
} from "./use-message-queue"
import type { PromptDraft } from "@/lib/types"

function draft(text: string): PromptDraft {
  return { blocks: [{ type: "text", text }], displayText: text }
}

function texts(q: { draft: PromptDraft }[]): string[] {
  return q.map((item) => item.draft.displayText)
}

describe("useMessageQueue bounce FIFO ordering", () => {
  it("requeueFront keeps a bounced head ahead of items behind it", () => {
    const { result } = renderHook(() => useMessageQueue())

    // Queue [A, B].
    act(() => result.current.enqueue(draft("A"), null))
    act(() => result.current.enqueue(draft("B"), null))
    expect(texts(result.current.queue)).toEqual(["A", "B"])

    // The auto-flush dequeues the head (A) and sends it.
    let dequeued: ReturnType<typeof result.current.dequeue>
    act(() => {
      dequeued = result.current.dequeue()
    })
    expect(dequeued?.draft.displayText).toBe("A")
    expect(texts(result.current.queue)).toEqual(["B"])

    // A bounces (TurnBusyError) → re-queued at the FRONT, NOT the tail, so it
    // retries before B. (Re-enqueuing at the tail here would yield [B, A] and
    // send B before A — the FIFO regression this guards against.)
    act(() => result.current.requeueFront(draft("A"), null))
    expect(texts(result.current.queue)).toEqual(["A", "B"])

    // The next flush therefore dequeues A again, not B.
    act(() => {
      dequeued = result.current.dequeue()
    })
    expect(dequeued?.draft.displayText).toBe("A")
  })

  it("enqueue still appends to the tail (front vs tail are distinct)", () => {
    const { result } = renderHook(() => useMessageQueue())
    act(() => result.current.enqueue(draft("A"), null))
    act(() => result.current.enqueue(draft("tail"), null))
    act(() => result.current.requeueFront(draft("front"), null))
    expect(texts(result.current.queue)).toEqual(["front", "A", "tail"])
  })

  it("getQueueLength reflects mutations SYNCHRONOUSLY (same tick, before re-render)", () => {
    const { result } = renderHook(() => useMessageQueue())
    // Multiple mutations within a single act() — getQueueLength must observe
    // each one immediately, without waiting for a React commit. This is what
    // the fork-send guard relies on: a draft re-queued by a same-tick bounce
    // is visible before the next render hides the fork affordance.
    act(() => {
      expect(result.current.getQueueLength()).toBe(0)
      result.current.enqueue(draft("A"), null)
      expect(result.current.getQueueLength()).toBe(1)
      result.current.requeueFront(draft("B"), null)
      expect(result.current.getQueueLength()).toBe(2)
      result.current.dequeue()
      expect(result.current.getQueueLength()).toBe(1)
    })
    // After commit the rendered queue matches the authoritative ref.
    expect(texts(result.current.queue)).toEqual(["A"])
    expect(result.current.getQueueLength()).toBe(1)
  })

  it("applies a valid reorder (a permutation of the live queue)", () => {
    const { result } = renderHook(() => useMessageQueue())
    act(() => result.current.enqueue(draft("A"), null))
    act(() => result.current.enqueue(draft("B"), null))
    const [a, b] = result.current.queue
    act(() => result.current.reorder([b, a]))
    expect(texts(result.current.queue)).toEqual(["B", "A"])
  })

  it("ignores a STALE reorder whose id set no longer matches (no resurrect/drop)", () => {
    const { result } = renderHook(() => useMessageQueue())
    act(() => result.current.enqueue(draft("A"), null))
    act(() => result.current.enqueue(draft("B"), null))
    const stale = [...result.current.queue].reverse() // snapshot of [A, B] → [B, A]
    // The queue changes (A dequeued) AFTER the drag snapshot was taken.
    act(() => result.current.dequeue())
    expect(texts(result.current.queue)).toEqual(["B"])
    // Applying the stale [B, A] order would resurrect A — it must be ignored.
    act(() => result.current.reorder(stale))
    expect(texts(result.current.queue)).toEqual(["B"])
  })

  it("ignores a reorder containing a duplicate id (would drop another item)", () => {
    const { result } = renderHook(() => useMessageQueue())
    act(() => result.current.enqueue(draft("A"), null))
    act(() => result.current.enqueue(draft("B"), null))
    const [a] = result.current.queue
    // [A, A] matches length + membership but is NOT a permutation — applying it
    // would duplicate A and drop B. Must be ignored.
    act(() => result.current.reorder([a, a]))
    expect(texts(result.current.queue)).toEqual(["A", "B"])
  })

  it("reorders the AUTHORITATIVE items, not the caller's stale objects", () => {
    const { result } = renderHook(() => useMessageQueue())
    act(() => result.current.enqueue(draft("A"), null))
    act(() => result.current.enqueue(draft("B"), null))
    const [a, b] = result.current.queue
    // A is edited AFTER the drag snapshot [a, b] was captured.
    act(() => result.current.updateItem(a.id, draft("A-edited")))
    // The stale reorder carries the OLD `a` object (draft "A"); the commit must
    // use the authoritative edited A (by id), only applying the requested order.
    act(() => result.current.reorder([b, a]))
    expect(texts(result.current.queue)).toEqual(["B", "A-edited"])
  })
})

describe("useMessageQueue persistence (offline survival across reload)", () => {
  const KEY = "codeg:msg-queue:v1:42"
  beforeEach(() => localStorage.clear())
  afterEach(() => localStorage.clear())

  it("does not touch localStorage without a persistKey", () => {
    const { result } = renderHook(() => useMessageQueue())
    act(() => result.current.enqueue(draft("hi"), null))
    expect(result.current.queue).toHaveLength(1)
    expect(localStorage.getItem(KEY)).toBeNull()
  })

  it("persists the queue and rehydrates it on a fresh mount (reload)", () => {
    const first = renderHook(() => useMessageQueue(42))
    act(() =>
      first.result.current.enqueue(
        draft("offline message"),
        "modeA",
        "optimistic-stable"
      )
    )
    expect(JSON.parse(localStorage.getItem(KEY)!)).toHaveLength(1)

    // A fresh mount (page reload during the outage) restores the queue.
    const reloaded = renderHook(() => useMessageQueue(42))
    expect(texts(reloaded.result.current.queue)).toEqual(["offline message"])
    expect(reloaded.result.current.queue[0].modeId).toBe("modeA")
    expect(reloaded.result.current.queue[0].clientMessageId).toBe(
      "optimistic-stable"
    )
  })

  it("preserves restore-waiting text, image, attachment, and stable id", () => {
    const pendingDraft: PromptDraft = {
      displayText: "inspect these files",
      blocks: [
        { type: "text", text: "inspect these files" },
        {
          type: "image",
          data: "base64-image",
          mime_type: "image/png",
          uri: "file:///uploads/screenshot.png",
        },
        {
          type: "resource",
          uri: "file:///uploads/spec.pdf",
          mime_type: "application/pdf",
          blob: "base64-pdf",
        },
      ],
    }
    const first = renderHook(() => useMessageQueue(42))
    act(() =>
      first.result.current.enqueue(
        pendingDraft,
        "modeA",
        "optimistic-with-attachments",
        { state: "waiting_session_restore" }
      )
    )

    const reloaded = renderHook(() => useMessageQueue(42))
    const restored = reloaded.result.current.peekNext("prompt")
    expect(restored).toMatchObject({
      clientMessageId: "optimistic-with-attachments",
      state: "waiting_session_restore",
      draft: pendingDraft,
    })
  })

  it("compacts uploaded image bytes only in persisted remote copies", () => {
    const uploadedDraft: PromptDraft = {
      displayText: "with image",
      blocks: [
        {
          type: "image",
          data: "large-base64-image",
          mime_type: "image/png",
          uri: "file:///uploads/image.png",
        },
        {
          type: "resource",
          uri: "file:///uploads/embedded.png",
          mime_type: "image/png",
          blob: "large-base64-resource",
        },
      ],
    }
    const { result } = renderHook(() =>
      useMessageQueue(42, { compactUploadedImages: true })
    )
    act(() =>
      result.current.enqueue(uploadedDraft, null, "optimistic-uploaded", {
        state: "waiting_session_restore",
      })
    )

    expect(result.current.queue[0].draft.blocks).toEqual(uploadedDraft.blocks)
    const [persisted] = JSON.parse(localStorage.getItem(KEY)!)
    expect(persisted.draft.blocks[0]).toMatchObject({
      data: "",
      uri: "file:///uploads/image.png",
    })
    expect(persisted.draft.blocks[1]).toMatchObject({
      blob: "",
      uri: "file:///uploads/embedded.png",
    })
  })

  it("auto-retries a stable id after the connection recovers", () => {
    const { result } = renderHook(() => useMessageQueue(42))
    act(() =>
      result.current.enqueue(draft("maybe accepted"), null, "stable-retry", {
        state: "waiting_connection",
      })
    )
    expect(result.current.peekNext("prompt")?.clientMessageId).toBe(
      "stable-retry"
    )
  })

  it("persists guide identity and only converts it to a prompt explicitly", () => {
    const target = {
      sessionId: "session-original",
      connectionId: "connection-original",
      userMessageId: "optimistic-original-task",
    }
    const first = renderHook(() => useMessageQueue(42))
    act(() =>
      first.result.current.enqueue(
        draft("change direction"),
        null,
        "optimistic-guide",
        {
          intent: "guide",
          state: "waiting_session_restore",
          guideTarget: target,
        }
      )
    )

    const reloaded = renderHook(() => useMessageQueue(42))
    expect(reloaded.result.current.peekNext("guide")).toMatchObject({
      intent: "guide",
      clientMessageId: "optimistic-guide",
      guideTarget: target,
    })
    const id = reloaded.result.current.queue[0].id
    act(() =>
      reloaded.result.current.markState(
        id,
        "expired_guide",
        "original task ended"
      )
    )
    expect(reloaded.result.current.peekNext("prompt")).toBeUndefined()
    act(() => reloaded.result.current.convertGuideToPrompt(id))
    expect(reloaded.result.current.peekNext("prompt")).toMatchObject({
      intent: "prompt",
      guideTarget: null,
      clientMessageId: "optimistic-guide",
    })
  })

  it("preserves client_message_id across dequeue and Busy requeue", () => {
    const { result } = renderHook(() => useMessageQueue())
    act(() =>
      result.current.enqueue(draft("image prompt"), null, "optimistic-image")
    )
    let item: ReturnType<typeof result.current.dequeue>
    act(() => {
      item = result.current.dequeue()
    })
    act(() =>
      result.current.requeueFront(
        item!.draft,
        item!.modeId,
        item!.clientMessageId
      )
    )
    expect(result.current.queue[0].clientMessageId).toBe("optimistic-image")
  })

  it("backfills stable ids for legacy persisted queue entries", () => {
    localStorage.setItem(
      KEY,
      JSON.stringify([
        { id: "legacy-id", draft: draft("legacy"), modeId: null },
      ])
    )
    const { result } = renderHook(() => useMessageQueue(42))
    expect(result.current.queue[0].clientMessageId).toBe("optimistic-legacy-id")
  })

  it("clears the persisted slot when the queue drains", () => {
    const { result } = renderHook(() => useMessageQueue(42))
    act(() => result.current.enqueue(draft("m1"), null))
    expect(localStorage.getItem(KEY)).toBeTruthy()
    act(() => {
      result.current.dequeue()
    })
    expect(result.current.queue).toHaveLength(0)
    expect(localStorage.getItem(KEY)).toBeNull()
  })
})

describe("deferred guide target safety", () => {
  const target = {
    sessionId: "session-original",
    connectionId: "connection-original",
    userMessageId: "optimistic-original-task",
  }

  it("accepts a guide only for the same still-running task", () => {
    expect(
      isQueuedGuideTargetCurrent(target, {
        sessionId: "session-original",
        connectionId: "connection-restored",
        pendingUserMessageId: "optimistic-original-task",
        status: "prompting",
      })
    ).toBe(true)
  })

  it("rejects a guide after the original task ended or a new task started", () => {
    expect(
      isQueuedGuideTargetCurrent(target, {
        sessionId: "session-original",
        connectionId: "connection-original",
        pendingUserMessageId: null,
        status: "connected",
      })
    ).toBe(false)
    expect(
      isQueuedGuideTargetCurrent(target, {
        sessionId: "session-original",
        connectionId: "connection-original",
        pendingUserMessageId: "optimistic-new-task",
        status: "prompting",
      })
    ).toBe(false)
  })
})
