"use client"

import { useCallback, useRef, useState } from "react"
import type { PromptDraft } from "@/lib/types"
import { randomUUID } from "@/lib/utils"

export type QueuedMessageIntent = "prompt" | "guide" | "fork" | "branch"
export type QueuedMessageState =
  | "queued"
  | "waiting_session_restore"
  | "waiting_connection"
  | "failed"
  | "expired_guide"

export interface QueuedGuideTarget {
  /** Historical ACP session the running turn belongs to. */
  sessionId: string | null
  /** Concrete live connection at the time the guide was composed. */
  connectionId: string | null
  /** Stable id of the original in-flight user prompt, when already observed. */
  userMessageId: string | null
}

export function isQueuedGuideTargetCurrent(
  target: QueuedGuideTarget,
  current: {
    sessionId: string | null
    connectionId: string | null
    pendingUserMessageId: string | null
    status: string | null
  }
): boolean {
  if (current.status !== "prompting") return false
  if (target.sessionId != null && target.sessionId !== current.sessionId) {
    return false
  }
  return target.userMessageId
    ? target.userMessageId === current.pendingUserMessageId
    : target.connectionId === current.connectionId
}

export interface QueuedMessage {
  id: string
  /** Stable id reused when this item is retried after Busy/reconnect. */
  clientMessageId: string
  draft: PromptDraft
  modeId: string | null
  intent: QueuedMessageIntent
  state: QueuedMessageState
  error: string | null
  guideTarget: QueuedGuideTarget | null
}

export interface QueueEnqueueOptions {
  intent?: QueuedMessageIntent
  state?: QueuedMessageState
  error?: string | null
  guideTarget?: QueuedGuideTarget | null
}

export interface UseMessageQueueOptions {
  /**
   * Web/remote images have already been uploaded to a server-side `file://`
   * URI. Omit their duplicate base64 only in the persisted copy so a large
   * image cannot exhaust localStorage; the live in-memory draft stays intact.
   */
  compactUploadedImages?: boolean
}

// Persist the queue per conversation so undelivered messages (e.g. a send that
// failed on a network blip, then re-queued) survive a page reload during the
// outage. Best-effort: on quota/serialization failure the in-memory queue stays
// authoritative, we just skip persistence.
function queueStorageKey(
  persistKey: string | number | null | undefined
): string | null {
  return persistKey != null ? `codeg:msg-queue:v1:${persistKey}` : null
}

export const QUEUE_BRANCH_CREATION_EVENT =
  "codeg:queue-conversation-branch-creation"

export interface QueueBranchCreationRequest {
  conversationId: number
  requestId: string
  modeId: string | null
}

/** Ask the mounted conversation tab to persist a Create Branch operation in
 * the same FIFO as Fork & Send. Returns true when a tab accepted the request;
 * callers may fall back to the direct API when no tab runtime is mounted. */
export function queueConversationBranchCreation(
  request: QueueBranchCreationRequest
): boolean {
  if (typeof window === "undefined") return false
  const event = new CustomEvent<QueueBranchCreationRequest>(
    QUEUE_BRANCH_CREATION_EVENT,
    { detail: request, cancelable: true }
  )
  window.dispatchEvent(event)
  return event.defaultPrevented
}

function loadPersistedQueue(storageKey: string | null): QueuedMessage[] {
  if (!storageKey || typeof window === "undefined") return []
  try {
    const raw = localStorage.getItem(storageKey)
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    return parsed
      .filter(
        (
          x
        ): x is Omit<QueuedMessage, "clientMessageId"> & {
          clientMessageId?: string
        } =>
          !!x &&
          typeof x === "object" &&
          typeof (x as QueuedMessage).id === "string" &&
          !!(x as QueuedMessage).draft
      )
      .map((item) => ({
        ...item,
        clientMessageId:
          typeof item.clientMessageId === "string" &&
          item.clientMessageId.length > 0
            ? item.clientMessageId
            : `optimistic-${item.id}`,
        intent:
          item.intent === "guide"
            ? "guide"
            : item.intent === "fork"
              ? "fork"
              : item.intent === "branch"
                ? "branch"
                : "prompt",
        state: isQueuedMessageState(item.state) ? item.state : "queued",
        error: typeof item.error === "string" ? item.error : null,
        guideTarget: isQueuedGuideTarget(item.guideTarget)
          ? item.guideTarget
          : null,
      }))
  } catch {
    return []
  }
}

function isQueuedMessageState(value: unknown): value is QueuedMessageState {
  return (
    value === "queued" ||
    value === "waiting_session_restore" ||
    value === "waiting_connection" ||
    value === "failed" ||
    value === "expired_guide"
  )
}

function isQueuedGuideTarget(value: unknown): value is QueuedGuideTarget {
  if (!value || typeof value !== "object") return false
  const target = value as Partial<QueuedGuideTarget>
  return (
    (target.sessionId == null || typeof target.sessionId === "string") &&
    (target.connectionId == null || typeof target.connectionId === "string") &&
    (target.userMessageId == null || typeof target.userMessageId === "string")
  )
}

function compactDraftForPersistence(draft: PromptDraft): PromptDraft {
  return {
    ...draft,
    blocks: draft.blocks.map((block) => {
      if (
        block.type === "image" &&
        block.data.length > 0 &&
        block.uri?.startsWith("file://")
      ) {
        return { ...block, data: "" }
      }
      if (
        block.type === "resource" &&
        typeof block.blob === "string" &&
        block.blob.length > 0 &&
        block.mime_type?.startsWith("image/") &&
        block.uri.startsWith("file://")
      ) {
        return { ...block, blob: "" }
      }
      return block
    }),
  }
}

function persistQueue(
  storageKey: string | null,
  queue: QueuedMessage[],
  compactUploadedImages: boolean
): void {
  if (!storageKey || typeof window === "undefined") return
  try {
    if (queue.length === 0) {
      localStorage.removeItem(storageKey)
    } else {
      const persisted = compactUploadedImages
        ? queue.map((item) => ({
            ...item,
            draft: compactDraftForPersistence(item.draft),
          }))
        : queue
      localStorage.setItem(storageKey, JSON.stringify(persisted))
    }
  } catch {
    /* quota / serialization — keep the in-memory queue as source of truth */
  }
}

/** Persist a message into another conversation's queue before opening its tab.
 * Used by Fork & Send: branch creation and the user's first prompt are two
 * durable, idempotent operations, so a reload between them cannot lose or
 * duplicate the prompt. */
export function enqueuePersistedMessageForConversation(
  persistKey: string | number,
  item: QueuedMessage,
  compactUploadedImages = false
): void {
  const storageKey = queueStorageKey(persistKey)
  const current = loadPersistedQueue(storageKey)
  if (
    current.some(
      (candidate) => candidate.clientMessageId === item.clientMessageId
    )
  ) {
    return
  }
  persistQueue(storageKey, [...current, item], compactUploadedImages)
}

export interface UseMessageQueueReturn {
  queue: QueuedMessage[]
  enqueue: (
    draft: PromptDraft,
    modeId: string | null,
    clientMessageId?: string,
    options?: QueueEnqueueOptions
  ) => void
  /**
   * Put a draft back at the FRONT of the queue. Used when an auto-flushed item
   * was dequeued, sent, and bounced (TurnBusyError): it must return to the head
   * so it retries before items that were already behind it (FIFO preserved).
   */
  requeueFront: (
    draft: PromptDraft,
    modeId: string | null,
    clientMessageId?: string
  ) => void
  dequeue: () => QueuedMessage | undefined
  /** First auto-sendable item of the requested intent, read synchronously. */
  peekNext: (intent: QueuedMessageIntent) => QueuedMessage | undefined
  remove: (id: string) => void
  markState: (
    id: string,
    state: QueuedMessageState,
    error?: string | null
  ) => void
  retry: (id: string) => void
  convertGuideToPrompt: (id: string) => void
  reorder: (items: QueuedMessage[]) => void
  updateItem: (id: string, draft: PromptDraft) => void
  /**
   * The queue length, read SYNCHRONOUSLY from the authoritative ref — it
   * reflects the same-tick result of an enqueue/requeue/dequeue, before React
   * commits the next render. Callers gating on "is the queue non-empty right
   * now" (the fork-send guard, the direct-send routing) must use this rather
   * than `queue.length` (which lags a render).
   */
  getQueueLength: () => number
  editingItemId: string | null
  startEditing: (id: string) => void
  cancelEditing: () => void
}

export function useMessageQueue(
  // When provided, the queue is persisted to localStorage under this key
  // (typically the conversation id) so it survives a reload during an outage.
  // Pass a STABLE key — a changing key would reload from the new slot and drop
  // in-memory items.
  persistKey?: string | number | null,
  options?: UseMessageQueueOptions
): UseMessageQueueReturn {
  const storageKey = queueStorageKey(persistKey)
  const [queue, setQueue] = useState<QueuedMessage[]>(() =>
    loadPersistedQueue(storageKey)
  )
  const [editingItemId, setEditingItemId] = useState<string | null>(null)
  // Authoritative copy of the queue, updated SYNCHRONOUSLY by every mutation
  // (before the React state commit). Reads that must observe the same-tick
  // result of a mutation — the fork-send guard and the direct-send queue
  // routing — go through this ref / `getQueueLength`, NOT the `queue` state
  // (which lags until React commits) and NOT a passive-effect-synced mirror
  // (which lags a full render). Without this, a bounce that re-queues a draft
  // leaves a window where the guard still sees an empty queue.
  const queueRef = useRef<QueuedMessage[]>(queue)

  // Update the authoritative ref first, then schedule the render. A plain value
  // (not a functional updater) is correct because `queueRef.current` is always
  // the latest committed value.
  const commit = useCallback(
    (next: QueuedMessage[]) => {
      queueRef.current = next
      setQueue(next)
      persistQueue(storageKey, next, options?.compactUploadedImages ?? false)
    },
    [options?.compactUploadedImages, storageKey]
  )

  const enqueue = useCallback(
    (
      draft: PromptDraft,
      modeId: string | null,
      clientMessageId = `optimistic-${randomUUID()}`,
      options?: QueueEnqueueOptions
    ) => {
      if (
        queueRef.current.some(
          (item) => item.clientMessageId === clientMessageId
        )
      ) {
        return
      }
      commit([
        ...queueRef.current,
        {
          id: randomUUID(),
          clientMessageId,
          draft,
          modeId,
          intent: options?.intent ?? "prompt",
          state: options?.state ?? "queued",
          error: options?.error ?? null,
          guideTarget: options?.guideTarget ?? null,
        },
      ])
    },
    [commit]
  )

  const requeueFront = useCallback(
    (
      draft: PromptDraft,
      modeId: string | null,
      clientMessageId = `optimistic-${randomUUID()}`
    ) => {
      commit([
        {
          id: randomUUID(),
          clientMessageId,
          draft,
          modeId,
          intent: "prompt",
          state: "queued",
          error: null,
          guideTarget: null,
        },
        ...queueRef.current,
      ])
    },
    [commit]
  )

  const dequeue = useCallback((): QueuedMessage | undefined => {
    const current = queueRef.current
    if (current.length === 0) return undefined
    commit(current.slice(1))
    return current[0]
  }, [commit])

  const peekNext = useCallback(
    (intent: QueuedMessageIntent): QueuedMessage | undefined =>
      queueRef.current.find(
        (item) =>
          item.intent === intent &&
          (item.state === "queued" ||
            item.state === "waiting_session_restore" ||
            item.state === "waiting_connection")
      ),
    []
  )

  const remove = useCallback(
    (id: string) => {
      if (editingItemId === id) {
        setEditingItemId(null)
      }
      commit(queueRef.current.filter((item) => item.id !== id))
    },
    [commit, editingItemId]
  )

  const markState = useCallback(
    (id: string, state: QueuedMessageState, error: string | null = null) => {
      commit(
        queueRef.current.map((item) =>
          item.id === id ? { ...item, state, error } : item
        )
      )
    },
    [commit]
  )

  const retry = useCallback(
    (id: string) => {
      commit(
        queueRef.current.map((item) =>
          item.id === id ? { ...item, state: "queued", error: null } : item
        )
      )
    },
    [commit]
  )

  const convertGuideToPrompt = useCallback(
    (id: string) => {
      commit(
        queueRef.current.map((item) =>
          item.id === id
            ? {
                ...item,
                intent: "prompt",
                state: "queued",
                error: null,
                guideTarget: null,
              }
            : item
        )
      )
    },
    [commit]
  )

  const reorder = useCallback(
    (items: QueuedMessage[]) => {
      // Apply a reorder ONLY if it is a true permutation of the live queue, and
      // rebuild it from the AUTHORITATIVE items rather than the caller's
      // (possibly stale) objects. A drag emission carries the queue order from
      // the render it began in; if the live queue changed since (dequeue /
      // requeue / remove / updateItem), the dragged array is stale. Reject any
      // length mismatch, unknown id, or repeated id (e.g. `[A, A]` would
      // otherwise drop `B` and duplicate `A`); commit the current item objects
      // in the requested order so a concurrent `updateItem` isn't clobbered.
      const current = queueRef.current
      if (items.length !== current.length) return
      const byId = new Map(current.map((item) => [item.id, item]))
      const seen = new Set<string>()
      const next: QueuedMessage[] = []
      for (const item of items) {
        const authoritative = byId.get(item.id)
        if (!authoritative || seen.has(item.id)) return
        seen.add(item.id)
        next.push(authoritative)
      }
      commit(next)
    },
    [commit]
  )

  const updateItem = useCallback(
    (id: string, draft: PromptDraft) => {
      commit(
        queueRef.current.map((item) =>
          item.id === id ? { ...item, draft } : item
        )
      )
      setEditingItemId(null)
    },
    [commit]
  )

  const getQueueLength = useCallback(() => queueRef.current.length, [])

  const startEditing = useCallback((id: string) => {
    setEditingItemId(id)
  }, [])

  const cancelEditing = useCallback(() => {
    setEditingItemId(null)
  }, [])

  return {
    queue,
    enqueue,
    requeueFront,
    dequeue,
    peekNext,
    remove,
    markState,
    retry,
    convertGuideToPrompt,
    reorder,
    updateItem,
    getQueueLength,
    editingItemId,
    startEditing,
    cancelEditing,
  }
}
