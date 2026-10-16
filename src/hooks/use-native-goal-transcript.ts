"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import type { SessionGoalState } from "@/lib/api"
import { useConversationRuntimeStore } from "@/stores/conversation-runtime-store"

const READ_INTERVAL_MS = 1500
const ERROR_RETRY_MS = 5000

/** ACP Goal continuations stream outside session/prompt. Keep their persisted
 * projection visible without grafting them onto the previous foreground turn.
 * The existing reader preserves paged history and reads a bounded recent tail.
 * After transitions settle, quiet Goals cost no transcript reads; token bursts
 * share one read per interval.
 */
export function useNativeGoalTranscript(
  connectionId: string | null,
  conversationId: number,
  active: boolean
) {
  const currentConnection = useRef(connectionId)
  const previous = useRef<{ key: string; status: string | null } | null>(null)
  const lastRead = useRef({ key: "", at: 0 })
  const [observed, setObserved] = useState<{
    connectionId: string
    status: string | null
  } | null>(null)

  useEffect(() => {
    currentConnection.current = connectionId
    return () => {
      currentConnection.current = null
    }
  }, [connectionId])

  const observeGoal = useCallback(
    (snapshot: SessionGoalState) => {
      if (!connectionId || currentConnection.current !== connectionId) return
      const status = snapshot.goal?.status ?? null
      setObserved((prev) =>
        prev?.connectionId === connectionId && prev.status === status
          ? prev
          : { connectionId, status }
      )
    },
    [connectionId]
  )

  useEffect(() => {
    if (
      !active ||
      !connectionId ||
      observed?.connectionId !== connectionId
    )
      return
    const key = `${connectionId}:${conversationId}`
    const last = previous.current
    previous.current = { key, status: observed.status }
    if (observed.status === null && (last?.key !== key || last.status === null))
      return

    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined
    // A native transition also needs a trailing read: the final transcript
    // record can be flushed after its last wire delta, including on pause/end.
    let settlingReads = 2
    const schedule = (minimumDelay = READ_INTERVAL_MS) => {
      if (cancelled || document.visibilityState === "hidden") return
      const session = useConversationRuntimeStore
        .getState()
        .byConversationId.get(conversationId)
      const interval = session?.detailError ? ERROR_RETRY_MS : READ_INTERVAL_MS
      const since = lastRead.current.key === key ? lastRead.current.at : 0
      timer = setTimeout(
        read,
        Math.max(minimumDelay, since + interval - Date.now())
      )
    }
    const read = () => {
      if (cancelled || document.visibilityState === "hidden") return
      const runtime = useConversationRuntimeStore.getState()
      const session = runtime.byConversationId.get(conversationId)
      if (
        session &&
        // A draft keeps its negative runtime key after binding. The reader
        // resolves its persisted id; do not move the session to a new key.
        (session.dbConversationId ?? conversationId) > 0 &&
        !session.detailLoading &&
        !session.loadingOlderTurns &&
        !session.activeTurnToken &&
        (settlingReads > 0 || session.pendingOutOfTurnContent)
      ) {
        lastRead.current = { key, at: Date.now() }
        settlingReads = Math.max(0, settlingReads - 1)
        runtime.actions.refetchDetail(conversationId, { preserveLive: true })
      }
      if (observed.status !== null || settlingReads > 0) schedule()
    }
    const onVisibility = () => {
      clearTimeout(timer)
      schedule(250)
    }
    document.addEventListener("visibilitychange", onVisibility)
    schedule(250)
    return () => {
      cancelled = true
      clearTimeout(timer)
      document.removeEventListener("visibilitychange", onVisibility)
    }
  }, [active, connectionId, conversationId, observed])

  return observeGoal
}
