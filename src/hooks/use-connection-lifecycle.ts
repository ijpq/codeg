"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { useAcpActions } from "@/contexts/acp-connections-context"
import { useTaskContext } from "@/contexts/task-context"
import { useConnection, type UseConnectionReturn } from "@/hooks/use-connection"
import { extractAppCommandError } from "@/lib/app-error"
import { isNetworkOrOfflineError } from "@/lib/network-error"
import { isSessionRestorePendingError } from "@/lib/session-restore"
import { TurnBusyError } from "@/lib/turn-busy"
import { type AgentType, type PromptDraft } from "@/lib/types"
import { getAgentLabel } from "@/lib/custom-agents"

interface UseConnectionLifecycleOptions {
  contextKey: string
  agentType: AgentType
  isActive: boolean
  workingDir?: string
  sessionId?: string
  /**
   * Persisted conversation id (when known). Passed to `connect()` so it can
   * discover and attach to a live connection another client already owns
   * (cross-client viewing) instead of always spawning a fresh agent.
   */
  conversationId?: number
  /**
   * Read at unmount-cleanup time: true when the component is unmounting
   * because the view is being REPARENTED (its tab moved between split groups /
   * an unsplit merged it), not closed. A transient unmount must not
   * disconnect — the remounted instance re-attaches to the same live
   * connection under the same contextKey.
   */
  isTransientUnmount?: () => boolean
}

export interface UseConnectionLifecycleReturn {
  conn: UseConnectionReturn
  modeLoading: boolean
  configOptionsLoading: boolean
  selectorsLoading: boolean
  /** True while the backend-owned persisted-session restore flight is pending. */
  restorePending: boolean
  autoConnectError: string | null
  handleFocus: () => void
  handleSend: (
    draft: PromptDraft,
    modeId?: string | null,
    opts?: {
      folderId?: number | null
      conversationId?: number | null
      clientMessageId?: string | null
      /**
       * Called when the backend rejected the send because a turn was already
       * in flight (a second, concurrent prompt). The caller re-queues the
       * draft instead of treating it as an error.
       */
      onTurnInProgress?: () => void
      /**
       * The historical ACP session exists but has not completed its exact
       * snapshot/identity handoff. The caller retains the draft in its durable
       * queue and retries after readiness instead of surfacing an error.
       */
      onSessionRestorePending?: () => void
      /** Fired only after `/acp_prompt` returned success (backend accepted). */
      onAccepted?: () => void
      /**
       * Called for every non-Busy failure after the error toast is shown.
       * `ambiguous=true` means the transport
       * response was lost and the backend may already have accepted the prompt;
       * callers must keep the optimistic message and reconcile by id.
       */
      onSendFailed?: (error: unknown, ambiguous: boolean) => void
    }
  ) => Promise<void>
  handleSetConfigOption: (configId: string, valueId: string) => void
  isCancelling: boolean
  handleCancel: () => void
  handleRespondPermission: (requestId: string, optionId: string) => void
}

/**
 * Unmount-cleanup decision: disconnect unless this client OWNS a connection
 * that still has work in flight — a prompting turn, or launched-but-unresolved
 * background tasks (async sub-agents / background shells). Disconnecting an
 * owner kills the agent CLI, and the background work with it; busy owners are
 * left to the idle sweeps, which exempt them until the work settles (or the
 * backend max-age valve expires it). Viewers always tear down: their
 * disconnect only detaches (never kills the owner's agent), and the sweep
 * skips viewers so leaving one attached would leak its subscription.
 * EXCEPT on a transient unmount (tab reparented across split groups, not
 * closed): the remounted view re-attaches to the same connection, so neither
 * owners nor viewers tear down.
 * Shares `isConnectionBusy` with the preview-replacement release
 * (`disconnectIfIdle`) so the two teardown paths can't drift apart.
 * Exported for tests.
 */
export function shouldReleaseSurfaceOnUnmount(args: {
  transientUnmount?: boolean
}): boolean {
  return !args.transientUnmount
}

function normalizeErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return String(error)
}

function isExpectedConnectError(error: unknown): boolean {
  if (!error || typeof error !== "object") return false
  return (error as { alerted?: unknown }).alerted === true
}

export function useConnectionLifecycle({
  contextKey,
  agentType,
  isActive,
  workingDir,
  sessionId,
  conversationId,
  isTransientUnmount,
}: UseConnectionLifecycleOptions): UseConnectionLifecycleReturn {
  const t = useTranslations("Folder.chat.connectionLifecycle")
  const { setActiveKey, touchActivity, releaseSurface } = useAcpActions()
  const { addTask, updateTask, removeTask } = useTaskContext()
  const conn = useConnection(contextKey)

  // Destructure stable callbacks (depend only on actions + contextKey)
  // vs. volatile derived state (status, modes, etc.)
  const {
    status,
    selectorsReady,
    connect: connConnect,
    sendPrompt,
    setMode: connSetMode,
    setConfigOption: connSetConfigOption,
    cancel: connCancel,
    refreshSnapshot: connRefreshSnapshot,
    reconnect: connReconnect,
    respondPermission: connRespondPermission,
    modes,
    configOptions,
    hasCachedSelectors,
  } = conn
  const isInteractiveStatus = status === "connected" || status === "prompting"
  const hasSelectorsData = modes !== null || configOptions !== null
  const effectiveSelectorsReady = selectorsReady || hasSelectorsData
  const selectorTaskIdRef = useRef<string | null>(null)
  // Visual-only loading indicators for selector chips.
  // Skip loading indicators when we have cached selectors — even if the
  // cache contains no modes/configOptions (the agent simply doesn't have
  // them), we already know what to show and don't need a loading state.
  const modeLoading =
    !hasCachedSelectors &&
    (status === "connecting" ||
      (isInteractiveStatus && !effectiveSelectorsReady))
  const configOptionsLoading =
    !hasCachedSelectors &&
    (status === "connecting" ||
      (isInteractiveStatus && !effectiveSelectorsReady))
  const [isCancelling, setIsCancelling] = useState(false)
  const cancelRequestInFlightRef = useRef(false)
  const cancelReconcileTimerRef = useRef<number | null>(null)
  useEffect(() => {
    if (status === "prompting") return
    // Status is an external ACP snapshot. Defer the local visual reset to the
    // subscription turn rather than cascading a synchronous render from the
    // effect body.
    const timer = window.setTimeout(() => {
      cancelRequestInFlightRef.current = false
      setIsCancelling(false)
      if (cancelReconcileTimerRef.current !== null) {
        window.clearTimeout(cancelReconcileTimerRef.current)
        cancelReconcileTimerRef.current = null
      }
    }, 0)
    return () => window.clearTimeout(timer)
  }, [status])
  useEffect(
    () => () => {
      if (cancelReconcileTimerRef.current !== null) {
        window.clearTimeout(cancelReconcileTimerRef.current)
      }
    },
    []
  )
  // Gate for send button: block until the backend session is fully
  // initialized (selectorsReady from the real backend event, not cache).
  const selectorsLoading = isInteractiveStatus && !selectorsReady
  const [lastAutoConnectError, setLastAutoConnectError] = useState<{
    contextKey: string
    agentType: AgentType
    message: string
  } | null>(null)
  const [restorePending, setRestorePending] = useState(false)
  const restoreGenerationRef = useRef(0)

  // Refs for auto-connect effect, which intentionally avoids volatile
  // dependencies to prevent reconnect loops. Synced via useEffect —
  // effects run in declaration order, so these are current before
  // the auto-connect effect reads them.
  const contextKeyRef = useRef(contextKey)
  useEffect(() => {
    contextKeyRef.current = contextKey
  }, [contextKey])
  const connConnectRef = useRef(connConnect)
  useEffect(() => {
    connConnectRef.current = connConnect
  }, [connConnect])
  const sessionIdRef = useRef(sessionId)
  useEffect(() => {
    sessionIdRef.current = sessionId
  }, [sessionId])
  const conversationIdRef = useRef(conversationId)
  useEffect(() => {
    conversationIdRef.current = conversationId
  }, [conversationId])
  const modeIdRef = useRef<string | null>(modes?.current_mode_id ?? null)
  useEffect(() => {
    modeIdRef.current = modes?.current_mode_id ?? null
  }, [modes?.current_mode_id])
  // Sync activeKey when this view is the active tab
  useEffect(() => {
    if (isActive && contextKey) {
      setActiveKey(contextKey)
      touchActivity(contextKey)
    }
  }, [isActive, contextKey, setActiveKey, touchActivity])

  // Auto-connect when tab becomes active and workingDir is available.
  // Depends on isActive + workingDir + agentType + persisted identities so
  // that a detail/runtime load resolving `sessionId` cannot strand the early
  // `session_id=None` connection. `connect()` deduplicates an identical target,
  // while a newly-resolved session/conversation id enters the backend atomic
  // restore path.
  //
  // The working-directory dependency ensures connections wait
  // for folder info to load (workingDir transitions from undefined →
  // folder.path), and so that changing folders or agents on an already-
  // connected tab triggers a reconnect. The context's connect() dedups
  // same-param calls and disconnects+reconnects when workingDir or
  // agentType differs. Status changes must NOT re-trigger this to avoid
  // infinite reconnect loops on transient errors.
  useEffect(() => {
    if (!isActive) return
    if (!workingDir) return
    let cancelled = false
    const generation = ++restoreGenerationRef.current
    const restoringPersistedCodex =
      agentType === "codex" &&
      conversationIdRef.current != null &&
      conversationIdRef.current > 0
    if (restoringPersistedCodex) {
      queueMicrotask(() => {
        if (!cancelled && restoreGenerationRef.current === generation) {
          setRestorePending(true)
        }
      })
    }
    connConnectRef
      .current(
        agentType,
        workingDir,
        sessionIdRef.current,
        conversationIdRef.current
      )
      .then(() => {
        if (!cancelled && restoreGenerationRef.current === generation) {
          setLastAutoConnectError(null)
        }
      })
      .catch((e: unknown) => {
        if (!cancelled && restoreGenerationRef.current === generation) {
          setLastAutoConnectError({
            contextKey: contextKeyRef.current,
            agentType,
            message: normalizeErrorMessage(e),
          })
        }
        if (!isExpectedConnectError(e)) {
          console.error("[ConnLifecycle] auto-connect:", e)
        }
      })
      .finally(() => {
        if (!cancelled && restoreGenerationRef.current === generation) {
          setRestorePending(false)
        }
      })
    return () => {
      cancelled = true
    }
  }, [isActive, workingDir, agentType, sessionId, conversationId])

  // Manage task status for connection progress
  const taskIdRef = useRef<string | null>(null)
  useEffect(() => {
    if (status === "connecting" || restorePending) {
      if (!taskIdRef.current) {
        const id = `acp-connect-${Date.now()}`
        taskIdRef.current = id
        const agent = getAgentLabel(agentType)
        addTask(
          id,
          t("tasks.connectingTitle", { agent }),
          t("tasks.connectingDescription")
        )
      }
      updateTask(taskIdRef.current, { status: "running" })
    } else if (status === "connected" || status === "prompting") {
      if (taskIdRef.current) {
        updateTask(taskIdRef.current, { status: "completed" })
        taskIdRef.current = null
      }
    } else if (status === "error") {
      if (taskIdRef.current) {
        updateTask(taskIdRef.current, {
          status: "failed",
          error: t("errors.connectionFailed"),
        })
        taskIdRef.current = null
      }
    } else if (status === "disconnected" || status === null) {
      if (taskIdRef.current) {
        removeTask(taskIdRef.current)
        taskIdRef.current = null
      }
    }
  }, [status, restorePending, addTask, updateTask, removeTask, agentType, t])

  const clearSelectorTask = useCallback(() => {
    if (selectorTaskIdRef.current) {
      removeTask(selectorTaskIdRef.current)
      selectorTaskIdRef.current = null
    }
  }, [removeTask])

  useEffect(() => {
    const isInteractive = status === "connected" || status === "prompting"
    if (!isInteractive) {
      clearSelectorTask()
      return
    }

    if (selectorsReady) {
      clearSelectorTask()
      return
    }

    if (!selectorTaskIdRef.current) {
      const id = `acp-session-init-${Date.now()}`
      selectorTaskIdRef.current = id
      const agent = getAgentLabel(agentType)
      addTask(
        id,
        t("tasks.initSessionTitle", { agent }),
        t("tasks.initSessionDescription")
      )
      updateTask(id, { status: "running" })
    }
  }, [
    status,
    selectorsReady,
    agentType,
    addTask,
    updateTask,
    clearSelectorTask,
    t,
  ])

  // Keep a ref to the non-destructive surface release so unmount never turns a
  // React/browser lifecycle event into a backend task interruption.
  const releaseSurfaceRef = useRef(releaseSurface)
  useEffect(() => {
    releaseSurfaceRef.current = releaseSurface
  }, [releaseSurface])
  const isTransientUnmountRef = useRef(isTransientUnmount)
  useEffect(() => {
    isTransientUnmountRef.current = isTransientUnmount
  }, [isTransientUnmount])

  // Clean up on unmount (e.g. tab closed) by detaching this frontend surface.
  // The backend connection is deliberately not stopped: React remounts,
  // WebSocket churn, and stale client status are not cancellation intent.
  // Backend activity/idle reconciliation owns eventual process cleanup.
  useEffect(() => {
    return () => {
      if (
        shouldReleaseSurfaceOnUnmount({
          transientUnmount: isTransientUnmountRef.current?.() === true,
        })
      ) {
        // A few isolated hook harnesses provide a deliberately minimal mocked
        // action object. Production providers always expose releaseSurface;
        // tolerate the old mock shape so unmount cannot mask the assertion the
        // test actually owns.
        if (typeof releaseSurfaceRef.current === "function") {
          releaseSurfaceRef.current(contextKeyRef.current).catch(() => {})
        }
      }
      // Task cleanup stays unconditional even on transient unmounts — the
      // remounted instance mints fresh task ids, so stale ones would orphan.
      if (taskIdRef.current) {
        removeTask(taskIdRef.current)
      }
      clearSelectorTask()
    }
  }, [removeTask, clearSelectorTask])

  const handleFocus = useCallback(() => {
    // Respect the caller's readiness gate — e.g. historical conversations
    // set isActive=false until the session's external_id resolves, to
    // avoid connecting with sessionId=undefined and orphaning context.
    if (!isActive) return
    touchActivity(contextKey)
    if (!status || status === "disconnected" || status === "error") {
      setLastAutoConnectError(null)
      connConnect(agentType, workingDir, sessionId, conversationId).catch(
        (e: unknown) => {
          if (!isExpectedConnectError(e)) {
            console.error("[ConnLifecycle] connect:", e)
          }
        }
      )
    }
  }, [
    isActive,
    agentType,
    workingDir,
    sessionId,
    conversationId,
    status,
    connConnect,
    contextKey,
    touchActivity,
  ])

  const autoConnectError =
    status === "connected" || status === "prompting"
      ? null
      : lastAutoConnectError?.contextKey === contextKey &&
          lastAutoConnectError.agentType === agentType
        ? lastAutoConnectError.message
        : null

  // sendPrompt, connCancel, connRespondPermission are stable (depend
  // only on actions + contextKey), so these callbacks are effectively stable.
  const handleSend = useCallback(
    (
      draft: PromptDraft,
      modeId?: string | null,
      opts?: {
        folderId?: number | null
        conversationId?: number | null
        clientMessageId?: string | null
        onTurnInProgress?: () => void
        onSessionRestorePending?: () => void
        onAccepted?: () => void
        /**
         * Called for every non-Busy failure. The boolean marks an ambiguous
         * network/offline loss, where the backend may already have accepted
         * the prompt and the optimistic message must remain visible.
         */
        onSendFailed?: (error: unknown, ambiguous: boolean) => void
      }
    ): Promise<void> => {
      touchActivity(contextKey)
      const onTurnInProgress = opts?.onTurnInProgress
      const onSessionRestorePending = opts?.onSessionRestorePending
      const onAccepted = opts?.onAccepted
      const onSendFailed = opts?.onSendFailed
      return (async () => {
        const currentModeId = modeIdRef.current
        if (modeId && modeId !== currentModeId) {
          await connSetMode(modeId)
          // Optimistically track selected mode to avoid duplicate set_mode
          // calls before CurrentModeUpdate arrives from the agent.
          modeIdRef.current = modeId
        }
        await sendPrompt(draft.blocks, opts)
        onAccepted?.()
      })().catch((e: unknown) => {
        if (e instanceof TurnBusyError) {
          // A turn was already in flight on the connection (another
          // co-controlling client, or a "prompting" status this client hadn't
          // observed yet). Not an error — the draft is re-queued by the caller
          // so it auto-sends when the current turn finishes.
          onTurnInProgress?.()
          return
        }
        if (isSessionRestorePendingError(e)) {
          onSessionRestorePending?.()
          return
        }
        console.error("[ConnLifecycle] sendPrompt:", e)
        // The composer already cleared itself (sends are fire-and-forget), so
        // without a toast a failed send — an oversized body 413'd by the
        // server, a hydration failure, a network drop — looks like the message
        // simply vanished. Surface the failure; prefer the structured backend
        // message over a bare stringification.
        const appError = extractAppCommandError(e)
        const message =
          appError?.message ??
          (e instanceof Error ? e.message : String(e ?? "unknown error"))
        toast.error(t("errors.sendPromptFailed", { error: message }))
        // Let the caller distinguish a transport loss (the backend may have
        // accepted the id) from a deterministic rejection.
        onSendFailed?.(e, isNetworkOrOfflineError(e))
      })
    },
    [connSetMode, sendPrompt, contextKey, touchActivity, t]
  )

  const handleCancel = useCallback(() => {
    // The button stays mounted while the first HTTP request is in flight, so a
    // double click previously invoked /acp_cancel twice. This synchronous ref
    // closes that render gap; the backend's run CAS covers other tabs/devices.
    if (cancelRequestInFlightRef.current) return
    cancelRequestInFlightRef.current = true
    setIsCancelling(true)
    connCancel()
      .then((result) => {
        if (
          !result ||
          result.outcome === "already_finished" ||
          result.outcome === "run_not_found"
        ) {
          cancelRequestInFlightRef.current = false
          setIsCancelling(false)
          return
        }
        // Attach/replay normally delivers the terminal event. This one-shot
        // authoritative snapshot check is the backstop for a lost event: wait
        // through the backend cancel deadline and its 10s reconciliation tick,
        // then hydrate the real state. A still-prompting or missing connection
        // is re-established through the existing targeted reconnect path.
        if (result.deadlineAt) {
          const deadlineMs = Date.parse(result.deadlineAt)
          const delay = Number.isFinite(deadlineMs)
            ? Math.max(0, deadlineMs - Date.now()) + 12_000
            : 37_000
          if (cancelReconcileTimerRef.current !== null) {
            window.clearTimeout(cancelReconcileTimerRef.current)
          }
          cancelReconcileTimerRef.current = window.setTimeout(() => {
            cancelReconcileTimerRef.current = null
            void connRefreshSnapshot()
              .then((authoritativeStatus) => {
                if (
                  authoritativeStatus === null ||
                  authoritativeStatus === "prompting"
                ) {
                  return connReconnect()
                }
                return false
              })
              .catch((error: unknown) => {
                console.warn("[ConnLifecycle] cancel reconciliation:", error)
                return connReconnect()
              })
          }, delay)
        }
      })
      .catch((e: unknown) => {
        cancelRequestInFlightRef.current = false
        setIsCancelling(false)
        console.error("[ConnLifecycle] cancel:", e)
        toast.error(
          e instanceof Error ? e.message : String(e ?? "Unable to stop task")
        )
      })
  }, [connCancel, connReconnect, connRefreshSnapshot])

  const handleSetConfigOption = useCallback(
    (configId: string, valueId: string) => {
      touchActivity(contextKey)
      connSetConfigOption(configId, valueId).catch((e: unknown) =>
        console.error("[ConnLifecycle] setConfigOption:", e)
      )
    },
    [connSetConfigOption, contextKey, touchActivity]
  )

  const handleRespondPermission = useCallback(
    (requestId: string, optionId: string) => {
      touchActivity(contextKey)
      connRespondPermission(requestId, optionId).catch((e: unknown) =>
        console.error("[ConnLifecycle] respondPermission:", e)
      )
    },
    [connRespondPermission, contextKey, touchActivity]
  )

  return {
    conn,
    modeLoading,
    configOptionsLoading,
    selectorsLoading,
    restorePending,
    autoConnectError,
    handleFocus,
    handleSend,
    handleSetConfigOption,
    isCancelling,
    handleCancel,
    handleRespondPermission,
  }
}
