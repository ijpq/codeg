"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import {
  getBusinessState,
  switchBusinessAgent,
  type BusinessConversationState,
} from "@/lib/business-conversation"
import {
  CONVERSATION_CHANGED_EVENT,
  type AgentType,
  type ConversationChange,
} from "@/lib/types"
import { getTransport } from "@/lib/transport"
import { getSavedPrefsForConnect } from "@/lib/selector-prefs-storage"

export function useBusinessConversation(
  conversationId: number | null,
  visible = true
) {
  const [state, setState] = useState<BusinessConversationState | null>(null)
  const [submitting, setSubmitting] = useState<number | null>(null)
  const [error, setError] = useState<{
    conversationId: number
    message: string
  } | null>(null)
  const generation = useRef(0)
  const activeId = useRef(conversationId)
  const requestInFlight = useRef(new Set<number>())
  activeId.current = conversationId
  const invalidate = useCallback(() => {
    generation.current++
  }, [])
  const accept = useCallback((next: BusinessConversationState | null) => {
    setState((previous) => {
      if (!next)
        return previous?.conversationId === activeId.current ? previous : null
      if (next.conversationId !== activeId.current) return previous
      if (
        previous?.conversationId === next.conversationId &&
        previous.revision > next.revision
      )
        return previous
      return next
    })
  }, [])
  const refresh = useCallback(async () => {
    if (conversationId == null) return
    const token = ++generation.current
    const next = await getBusinessState(conversationId)
    if (activeId.current === conversationId && token === generation.current)
      accept(next)
  }, [accept, conversationId])
  useEffect(() => {
    if (conversationId == null || !visible) return
    let disposed = false
    let timer: ReturnType<typeof setTimeout> | undefined
    let unlisten: (() => void) | undefined
    const disposeSubscription = getTransport()
      .subscribe<ConversationChange>(CONVERSATION_CHANGED_EVENT, (change) => {
        if (change.kind === "upsert" && change.summary.id === conversationId)
          void refresh().catch(() => {})
      })
      .then((dispose) => {
        if (disposed) dispose()
        else unlisten = dispose
      })
      .catch(() => {})
    void disposeSubscription
    const poll = async () => {
      try {
        await refresh()
      } catch {
        /* Visible request/retry owns errors. */
      }
      if (!disposed) timer = setTimeout(poll, 1500)
    }
    void poll()
    const onFocus = () => {
      void refresh().catch(() => {})
    }
    window.addEventListener("focus", onFocus)
    return () => {
      disposed = true
      invalidate()
      unlisten?.()
      clearTimeout(timer)
      window.removeEventListener("focus", onFocus)
    }
  }, [conversationId, invalidate, refresh, visible])
  const switchAgent = useCallback(
    async (targetAgent: AgentType) => {
      if (conversationId == null || requestInFlight.current.has(conversationId))
        return
      requestInFlight.current.add(conversationId)
      const cid = conversationId
      setSubmitting(cid)
      setError(null)
      ++generation.current
      try {
        const preferences = getSavedPrefsForConnect(targetAgent)
        const next = await switchBusinessAgent({
          conversationId: cid,
          requestId: crypto.randomUUID(),
          targetAgent,
          expectedRevision:
            state?.conversationId === cid ? state.revision : null,
          preferredModeId: preferences.modeId,
          preferredConfigValues: preferences.configValues ?? {},
        })
        if (activeId.current === cid) accept(next)
      } catch (e) {
        if (activeId.current === cid)
          setError({
            conversationId: cid,
            message: e instanceof Error ? e.message : String(e),
          })
        await refresh().catch(() => {})
      } finally {
        requestInFlight.current.delete(cid)
        setSubmitting((current) => (current === cid ? null : current))
      }
    },
    [accept, conversationId, refresh, state]
  )
  const current = state?.conversationId === conversationId ? state : null
  return {
    state: current,
    pending:
      (submitting !== null && submitting === conversationId) ||
      current?.switchStatus === "queued" ||
      current?.switchStatus === "preparing",
    error: error?.conversationId === conversationId ? error.message : null,
    switchAgent,
    refresh,
  }
}
