"use client"

import { useCallback, useRef, useState } from "react"
import type { PromptDraft } from "@/lib/types"
import { randomUUID } from "@/lib/utils"

export interface QueuedMessage {
  id: string
  /** Stable id reused when this item is retried after Busy/reconnect. */
  clientMessageId: string
  draft: PromptDraft
  modeId: string | null
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
      }))
  } catch {
    return []
  }
}

function persistQueue(storageKey: string | null, queue: QueuedMessage[]): void {
  if (!storageKey || typeof window === "undefined") return
  try {
    if (queue.length === 0) {
      localStorage.removeItem(storageKey)
    } else {
      localStorage.setItem(storageKey, JSON.stringify(queue))
    }
  } catch {
    /* quota / serialization — keep the in-memory queue as source of truth */
  }
}

export interface UseMessageQueueReturn {
  queue: QueuedMessage[]
  enqueue: (
    draft: PromptDraft,
    modeId: string | null,
    clientMessageId?: string
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
  remove: (id: string) => void
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
  persistKey?: string | number | null
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
      persistQueue(storageKey, next)
    },
    [storageKey]
  )

  const enqueue = useCallback(
    (
      draft: PromptDraft,
      modeId: string | null,
      clientMessageId = `optimistic-${randomUUID()}`
    ) => {
      commit([
        ...queueRef.current,
        { id: randomUUID(), clientMessageId, draft, modeId },
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
        { id: randomUUID(), clientMessageId, draft, modeId },
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

  const remove = useCallback(
    (id: string) => {
      if (editingItemId === id) {
        setEditingItemId(null)
      }
      commit(queueRef.current.filter((item) => item.id !== id))
    },
    [commit, editingItemId]
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
    remove,
    reorder,
    updateItem,
    getQueueLength,
    editingItemId,
    startEditing,
    cancelEditing,
  }
}
