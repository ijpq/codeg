"use client"

import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react"
import {
  getWebConnectionServerSnapshot,
  getWebConnectionSnapshot,
  subscribeWebConnection,
} from "@/lib/transport/web-connection-store"
import {
  AlertCircle,
  Download,
  FileCode,
  FileImage,
  FileText,
  Info,
  Loader2,
  Plus,
  RefreshCw,
  SquarePen,
  X,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  getCachedSelectors,
  useAcpActions,
  useAcpEvent,
} from "@/contexts/acp-connections-context"
import { useAcpAgents } from "@/hooks/use-acp-agents"
import { onTransportReconnect, subscribe } from "@/lib/platform"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useTabActions, useTabStore } from "@/contexts/tab-context"
import { groupOfTab, isReparentUnmount } from "@/stores/tab-store"
import { computeRects, leafIds } from "@/lib/tab-group-layout"
import { useTaskContext } from "@/contexts/task-context"
import { cn, randomUUID } from "@/lib/utils"
import { buildQuotedMarkdown } from "@/lib/message-quote"
import { useConnectionLifecycle } from "@/hooks/use-connection-lifecycle"
import {
  isQueuedGuideTargetCurrent,
  enqueuePersistedMessageForConversation,
  QUEUE_BRANCH_CREATION_EVENT,
  useMessageQueue,
  type QueueBranchCreationRequest,
  type QueuedMessage,
} from "@/hooks/use-message-queue"
import { MessageListView } from "@/components/message/message-list-view"
import {
  GoalControlProvider,
  type GoalControlValue,
} from "@/components/message/goal-control-context"
import { useAdvertisedGoalActions } from "@/hooks/use-goal-actions"
import { ConversationShell } from "@/components/chat/conversation-shell"
import { SessionConfigStaleBanner } from "@/components/chat/session-config-stale-banner"
import { PiProjectTrustBanner } from "@/components/chat/pi-project-trust-banner"
import { FeedbackNotesDisplay } from "@/components/chat/feedback-notes-display"
import { FeedbackDialog } from "@/components/chat/feedback-dialog"
import { AgentDiagnosticsDialog } from "@/components/settings/agent-diagnostics-dialog"
import { useFeedbackEnabled } from "@/hooks/use-feedback-enabled"
import { useSessionFeedback } from "@/hooks/use-session-feedback"
import { AgentSelector } from "@/components/chat/agent-selector"
import { ChatInput } from "@/components/chat/chat-input"
import { WelcomeHero, WelcomeTip } from "@/components/chat/welcome-hero"
import { QuickActions } from "@/components/chat/quick-actions"
import type { ComposerInjectContent } from "@/components/chat/message-input"
import { TileScrollContainer } from "@/components/conversations/tile-scroll-container"
import { GroupSplitHandle } from "@/components/conversations/group-split-handle"
import { OverlayHostHiddenProvider } from "@/components/ui/overlay-host-hidden"
import { ScrollArea } from "@/components/ui/scroll-area"
import { TabBar } from "@/components/tabs/tab-bar"
import { TabDragGhost } from "@/components/tabs/tab-drag-ghost"
import { useSidebarContext } from "@/contexts/sidebar-context"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"
import { useWorkspaceView } from "@/contexts/workspace-context"
import { useIsMobile } from "@/hooks/use-mobile"
import { usePlatform } from "@/hooks/use-platform"
import { useZoomLevel } from "@/hooks/use-appearance"
import { isDesktop, isLocalDesktop } from "@/lib/platform"
import { leftChromeReserve, rightChromeReserve } from "@/lib/window-chrome"
import {
  createConversationBranch,
  createChatConversation,
  createChatDir,
  createConversation,
  openSettingsWindow,
  submitSessionFeedback,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import { classifySteerFailure } from "@/lib/steer-errors"
import { isNetworkOrOfflineError } from "@/lib/network-error"
import {
  isSessionRestorePendingError,
  shouldAwaitHistoricalSessionDetail,
} from "@/lib/session-restore"
import {
  draftSupportsNativeSteer,
  shouldQueuePromptFailure,
} from "@/lib/prompt-delivery-state"
import {
  flushRetryDelayMs,
  isConnectionReady,
  shouldQueueDirectSend,
  shouldRejectDuplicateCreate,
} from "@/lib/queue-flush"
import {
  getConversationIdByExternalIdFromStore,
  getRuntimeSession,
  getTimelineTurns,
  useConversationRuntimeActions,
  useConversationRuntimeStore,
} from "@/stores/conversation-runtime-store"
import { useShallow } from "zustand/react/shallow"
import { useConversationDetail } from "@/hooks/use-conversation-detail"
import {
  extractUserImagesFromDraft,
  getPromptDraftDisplayText,
} from "@/lib/prompt-draft"
import {
  CONVERSATION_ARTIFACTS_CHANGED_EVENT,
  CONVERSATION_DELIVERABLES_CHANGED_EVENT,
  type AgentType,
  type ConversationArtifactsChanged,
  type ConversationDeliverablesChanged,
  type ContentBlock,
  type ConversationStatus,
  type EventEnvelope,
  type MessageTurn,
  type PlanApprovalAnswer,
  type PromptDraft,
  type QuestionAnswer,
  type UserMessageBlock,
} from "@/lib/types"
import { useRouter } from "next/navigation"
import {
  lastUserPromptText,
  type SessionFailureAction,
} from "@/lib/session-failures"
import { getAgentLabel } from "@/lib/custom-agents"
import {
  getSavedModeId,
  saveModePreference,
} from "@/lib/selector-prefs-storage"
import {
  adoptLegacyNewConversationDraft,
  buildConversationDraftStorageKey,
  buildNewConversationDraftStorageKey,
  clearMessageInputDraft,
  saveMessageInputDraft,
} from "@/lib/message-input-draft"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import {
  exportAsHtml,
  exportAsImage,
  exportAsMarkdown,
  ExportTooLongError,
} from "@/lib/export-conversation"
import { useExportLabels } from "@/lib/use-export-labels"
import { resolveActiveSessionDetails } from "./active-session-details"
import { ConversationDetailHeader } from "./conversation-detail-header"
import { SessionDetailsDialog } from "./session-details-dialog"

interface ConversationTabViewProps {
  tabId: string
  conversationId: number | null
  agentType: AgentType
  workingDir?: string
  isActive: boolean
  /** Fetch persisted history only when this keep-alive view is visible. In the
   *  normal single-tab layout that means the active tab; tiled layouts pass
   *  true for every visible tile. */
  shouldLoadDetail: boolean
  /** Mount the expensive message/composer subtree only while this tab is
   *  visible. The surrounding controller remains mounted for every open tab so
   *  background agent connections, turn completion, queues, and event handling
   *  keep working while hidden. */
  shouldRenderContent: boolean
  /** Drive the composer's flowing active-session border. True only for the
   *  active tab while several sessions are visible (tiled within a group
   *  and/or split across groups) — the places the flow serves as the "which
   *  session is active" cue. Distinct from `isActive`, which also governs
   *  auto-focus/connect and is true even for a lone session. */
  showActiveFlow: boolean
  reloadSignal: number
  /** The split group this view is rendered under. Used ONLY to tell a
   *  reparent (the tab moved to another group — remount, keep the connection)
   *  apart from a real teardown (pane switch / route change — disconnect). */
  groupId: string
}

function buildOptimisticUserTurnFromDraft(
  draft: PromptDraft,
  attachedResourcesFallback: string,
  clientMessageId = `optimistic-${randomUUID()}`
): MessageTurn {
  // `draft.displayText` is the composer's full Markdown, which already renders
  // every inline file/resource badge as a `[label](uri)` link (see
  // `referenceToMarkdown`). Re-appending the resource blocks here would duplicate
  // each attached file in the optimistic bubble, so the display text is used
  // as-is — images are the only out-of-band content left to add as blocks.
  const text = getPromptDraftDisplayText(draft, attachedResourcesFallback)

  const blocks: ContentBlock[] = []
  for (const image of extractUserImagesFromDraft(draft)) {
    blocks.push({
      type: "image",
      data: image.data,
      mime_type: image.mime_type,
      uri: image.uri ?? null,
    })
  }
  blocks.push({ type: "text", text })

  return {
    id: clientMessageId,
    role: "user",
    blocks,
    timestamp: new Date().toISOString(),
  }
}

/** Build a user `MessageTurn` from a broadcast `user_message` (event or
 *  snapshot `pending_user_message`). Used by cross-client VIEWERS to render the
 *  sender's prompt. The turn `id` is the broadcast `message_id` so the runtime
 *  reducer can dedup it idempotently. */
function buildUserTurnFromMessageBlocks(
  messageId: string,
  blocks: UserMessageBlock[]
): MessageTurn {
  const contentBlocks: ContentBlock[] = blocks.map((b) =>
    b.type === "image"
      ? { type: "image", data: b.data, mime_type: b.mime_type, uri: null }
      : { type: "text", text: b.text }
  )
  return {
    id: messageId,
    role: "user",
    blocks: contentBlocks,
    timestamp: new Date().toISOString(),
  }
}

function buildVirtualConversationId(seed: string): number {
  let hash = 0
  for (let i = 0; i < seed.length; i += 1) {
    hash = (hash * 31 + seed.charCodeAt(i)) | 0
  }
  const normalized = Math.abs(hash) + 1
  return -normalized
}

const ConversationTabView = memo(function ConversationTabView({
  tabId,
  conversationId,
  agentType,
  workingDir,
  isActive,
  shouldLoadDetail,
  shouldRenderContent,
  showActiveFlow,
  reloadSignal,
  groupId,
}: ConversationTabViewProps) {
  const t = useTranslations("Folder.conversation")
  const tWelcome = useTranslations("Folder.chat.welcomeInputPanel")
  const tDiag = useTranslations("DiagnosticsSettings")
  const sharedT = useTranslations("Folder.chat.shared")
  const tMessageList = useTranslations("Folder.chat.messageList")
  const refreshConversations = useAppWorkspaceStore(
    (s) => s.refreshConversations
  )
  const upsertFolder = useAppWorkspaceStore((s) => s.upsertFolder)
  // Subscribe to ONLY this tab's own row (identified by `tabId`), not the whole
  // `tabs` array — so a sibling tab changing, or a tab-switch (isActive rides in
  // as a prop), never re-renders this keep-alive panel. `find` returns the same
  // object reference across derives until this tab itself changes.
  const ownTab = useTabStore(
    (s) => s.tabs.find((tab) => tab.id === tabId) ?? null
  )
  // Resolve this panel's folder from ITS OWN tab, not the global active folder.
  // A keep-alive panel for a background tab must NOT re-render when the active
  // tab switches to a different folder. For the active tab this equals the old
  // `activeFolderId` (which is itself derived from the active tab's folderId via
  // `syncActiveFolderId`); it also avoids the brief post-switch window where the
  // global `activeFolderId` still lags on the previous tab's folder (same
  // rationale as the per-tab `workingDir` used for the connection below).
  const ownFolderId = ownTab?.folderId ?? null
  const folder = useAppWorkspaceStore((s) =>
    ownFolderId != null
      ? (s.allFolders.find((f) => f.id === ownFolderId) ?? null)
      : null
  )
  const folderId = ownFolderId ?? 0
  const {
    bindConversationTab,
    setChatDraftWorkingDir,
    setTabRuntimeConversationId,
    pinTab,
    openNewConversationTab,
    openTab,
    closeTab,
    confirmDraftAgent,
    setDraftAgentFromFallback,
  } = useTabActions()
  const {
    appendOptimisticTurn,
    removeOptimisticTurn,
    appendViewerUserTurn,
    completeTurn,
    refetchDetail,
    reconcileCompletedTurn,
    removeConversation,
    setAcpLoadError,
    setDbConversationId,
    setExternalId,
    setLiveMessage,
    setPendingCleanup,
    setPromptDeliveryPhase,
    setSyncState,
  } = useConversationRuntimeActions()
  const acpActions = useAcpActions()

  // Stable runtime session key — set once at mount, never changes.
  // For new conversations this is a virtual (negative) ID; for existing
  // conversations opened from the sidebar it equals the real DB ID.
  const [effectiveConversationId] = useState(
    () => conversationId ?? buildVirtualConversationId(`draft-${tabId}`)
  )
  const [createdConversationId, setCreatedConversationId] = useState<
    number | null
  >(null)
  const dbConversationId = conversationId ?? createdConversationId
  const [draftAgentType, setDraftAgentType] = useState<AgentType>(agentType)
  const selectedAgent = conversationId != null ? agentType : draftAgentType
  // Seed from localStorage so the React state reflects the user's saved
  // mode for this agent immediately on mount. Without this seed, a reuse-
  // path connect (idle window after a refresh, before the agent is GC'd)
  // would silently fall back to whatever `current_mode_id` the backend
  // happens to be on: `handleModeChange` updates only React state and
  // localStorage, not the agent — the agent gets synced inside
  // `handleSend` by diffing `modeId` against `modes.current_mode_id`.
  // A null seed here means that diff is "agent default vs null", which
  // resolves the displayed mode through `conn.modes.current_mode_id`
  // and never triggers the catch-up `setMode`.
  const [modeId, setModeId] = useState<string | null>(() =>
    getSavedModeId(agentType)
  )
  const [sendSignal, setSendSignal] = useState(0)
  const [agentsLoaded, setAgentsLoaded] = useState(false)
  const [usableAgentCount, setUsableAgentCount] = useState(0)
  const [composerDiagnosticsOpen, setComposerDiagnosticsOpen] = useState(false)
  const [agentConnectError, setAgentConnectError] = useState<string | null>(
    null
  )
  const [hasSentMessage, setHasSentMessage] = useState(false)
  // One inbox for everything pushed into this tab's composer from outside it:
  // welcome-page quick actions (replace) and quoted transcript selections
  // (append). Exactly one composer is mounted at a time — the welcome one or the
  // docked one — so a single slot can serve both.
  const [composerInject, setComposerInject] =
    useState<ComposerInjectContent | null>(null)

  const hasPersistedConversation = dbConversationId != null
  // Positive runtime keys belong to conversations opened from persisted
  // history and must resolve their DB-owned external session identity before
  // ACP connect. Draft-originated tabs keep a stable negative runtime key even
  // after their first prompt creates a DB row; those continue to use the live
  // SessionStarted identity and must not wait on a detail the virtual key never
  // fetches.
  const usesPersistedDetailIdentity = effectiveConversationId > 0

  // A folderless chat draft before its first send (chat tab, not yet persisted).
  // Used to trigger the eager scratch-dir prepare below, which gives the draft a
  // real workingDir so the ACP connection can spawn BEFORE the first send — the
  // composer is gated on `connected` like any normal conversation (no offline
  // compose). Once bound it has a persisted row + workingDir and this is false.
  const isChatDraft = useMemo(
    () => ownTab?.isChat === true && !hasPersistedConversation,
    [ownTab, hasPersistedConversation]
  )

  // Expose the runtime session key to the tab so the aux panel (Diff sidebar)
  // can look up live turns even before the DB conversation is created.
  useEffect(() => {
    if (effectiveConversationId !== conversationId) {
      setTabRuntimeConversationId(tabId, effectiveConversationId)
    }
  }, [
    tabId,
    effectiveConversationId,
    conversationId,
    setTabRuntimeConversationId,
  ])

  // Clear pendingCleanup when tab is (re)opened
  useEffect(() => {
    setPendingCleanup(effectiveConversationId, false)
  }, [effectiveConversationId, setPendingCleanup])

  const latestReloadSignal = useRef(reloadSignal)
  const pendingReloadState = useRef<{
    signal: number
    sawLoading: boolean
  } | null>(null)
  const dbConvIdRef = useRef<number | null>(conversationId)
  const mountedRef = useRef(true)
  const selectedAgentRef = useRef(selectedAgent)
  const createConversationPendingRef = useRef(false)
  const promptSubmitPendingRef = useRef(false)
  // Single-flight guard for the eager scratch-dir prepare (on chat-mode select).
  const prepareChatDirPendingRef = useRef(false)
  const sessionIdRef = useRef<string | null>(null)
  const syncCancelRef = useRef<(() => void) | null>(null)

  useEffect(() => {
    dbConvIdRef.current = dbConversationId
    // Bind the DB row id onto the runtime session when the two ids diverge
    // (draft-started tab: virtual runtime key, row created on first send).
    // `refetchDetail` on the runtime key fetches with this binding — without
    // it, a settle-driven refetch (background task finished) asks the backend
    // for the virtual id and silently fails, leaving stale live turns on
    // screen forever.
    if (
      dbConversationId != null &&
      dbConversationId !== effectiveConversationId
    ) {
      setDbConversationId(effectiveConversationId, dbConversationId)
    }
  }, [dbConversationId, effectiveConversationId, setDbConversationId])

  // An agent may publish final outputs before its reply completes. Refresh on
  // declarations immediately and again on terminal artifact settlement; the
  // latter is a redundant cross-device nudge after inference has committed.
  // The runtime→DB id binding also covers tabs begun under a virtual draft id.
  useEffect(() => {
    let disposed = false
    const unlistens: (() => void)[] = []
    const scheduleRefresh = (conversationId: number) => {
      if (conversationId === dbConvIdRef.current) {
        refetchDetail(effectiveConversationId, { preserveLive: true })
      }
    }

    const installSubscription = <T extends { conversation_id: number }>(
      event: string
    ) => {
      void subscribe<T>(event, (change) =>
        scheduleRefresh(change.conversation_id)
      ).then((dispose) => {
        if (disposed) dispose()
        else unlistens.push(dispose)
      })
    }
    installSubscription<ConversationDeliverablesChanged>(
      CONVERSATION_DELIVERABLES_CHANGED_EVENT
    )
    installSubscription<ConversationArtifactsChanged>(
      CONVERSATION_ARTIFACTS_CHANGED_EVENT
    )

    const offReconnect = onTransportReconnect(() => {
      const persistedId = dbConvIdRef.current
      if (persistedId == null) return
      if (connStatusRef.current === "prompting") {
        // Snapshot/detail refresh while the turn is active must retain the
        // optimistic image and any stream chunks already received.
        refetchDetail(effectiveConversationId, { preserveLive: true })
        return
      }
      // We may have missed both deltas and turn_complete while disconnected.
      // Poll the settled transcript and fold the authoritative final reply.
      const runtime = getRuntimeSession(effectiveConversationId)
      syncCancelRef.current?.()
      syncCancelRef.current = reconcileCompletedTurn(
        persistedId,
        effectiveConversationId,
        runtime?.activeTurnToken ?? null,
        () => setSendSignal((previous) => previous + 1)
      )
    })
    return () => {
      disposed = true
      unlistens.forEach((dispose) => dispose())
      offReconnect?.()
    }
  }, [effectiveConversationId, reconcileCompletedTurn, refetchDetail])

  useEffect(() => {
    selectedAgentRef.current = selectedAgent
  }, [selectedAgent])

  // Eagerly create the chat-mode scratch dir the moment this becomes an unbound
  // chat draft, so the ACP connection can spawn at a real cwd BEFORE the first
  // send — picking "no-folder mode" no longer leaves the agent unconnected.
  // Filesystem-only (writes no DB rows), so the lazy-conversation invariant
  // holds; the first send reuses this dir via createChatConversation(existingDir),
  // keeping the connection's cwd put across the bind. Single-flight and
  // self-disarming: once workingDir lands the guard flips false. openChatModeTab
  // clears workingDir on re-entry, so a fresh dir is prepared each time.
  useEffect(() => {
    if (!isActive || !isChatDraft || workingDir) return
    if (prepareChatDirPendingRef.current) return
    prepareChatDirPendingRef.current = true
    void (async () => {
      try {
        const res = await createChatDir()
        if (mountedRef.current) {
          setChatDraftWorkingDir(tabId, res.path)
        }
      } catch (e) {
        // The composer is gated on a live connection (no offline compose), and
        // the connection needs this scratch dir. If the mkdir fails the draft
        // would otherwise sit with a permanently disabled composer and no
        // explanation — surface it on the welcome screen's error banner so the
        // user can re-enter chat mode to retry.
        console.error("[ConversationTabView] prepare chat dir:", e)
        if (mountedRef.current) {
          setAgentConnectError(tWelcome("prepareSessionFailed"))
        }
      } finally {
        prepareChatDirPendingRef.current = false
      }
    })()
  }, [
    isActive,
    isChatDraft,
    workingDir,
    tabId,
    setChatDraftWorkingDir,
    tWelcome,
  ])

  // Sync the agentType prop into draftAgentType for draft tabs. The prop
  // changes when openNewConversationTab re-points an existing draft at a
  // different folder's default agent (or when any other external mutation
  // updates tab.agentType). Without this mirror, the local draftAgentType
  // would stay frozen at its mount value and the UI/connection would not
  // follow. Persisted conversations read agentType directly from the prop
  // via selectedAgent, so they are unaffected.
  useEffect(() => {
    if (conversationId != null) return
    if (agentType === selectedAgentRef.current) return
    setDraftAgentType(agentType)
    setModeId(getSavedModeId(agentType))
    setAgentConnectError(null)
  }, [agentType, conversationId])

  const {
    detail,
    loading: detailLoading,
    error: detailError,
    acpLoadError,
    hasEarlierHistory,
    earlierHistoryLoading,
    earlierHistoryError,
    loadEarlierHistory,
  } = useConversationDetail(effectiveConversationId, {
    enabled: shouldLoadDetail,
    // The DB detail owns the external session identity used by historical ACP
    // restore. Do not let existing optimistic/live runtime data suppress this
    // first read; retain those buffers while the detail is folded in.
    preserveLiveOnInitialFetch: usesPersistedDetailIdentity,
  })

  // Subscribe to only the fields this panel actually reads from its runtime
  // session — NOT the whole session object. The live-message sink rewrites the
  // session object on every streaming batch (~60/s, via SET_LIVE_MESSAGE); a
  // whole-object selector here would re-render this keep-alive panel (and the
  // composer subtree it wraps) on every streaming token, even though neither of
  // these two fields changes mid-stream. `useShallow` keeps the returned slice
  // reference-stable across batches, so the panel re-renders only when one of
  // them actually changes. (message-list-view subscribes to the session's
  // liveMessage separately to render the live stream; the context indicator
  // reads its own session stats from the runtime store directly.)
  const { externalId: runtimeExternalId, syncState: runtimeSyncState } =
    useConversationRuntimeStore(
      useShallow((s) => {
        const session = s.byConversationId.get(effectiveConversationId)
        return {
          externalId: session?.externalId ?? null,
          syncState: session?.syncState ?? "idle",
        }
      })
    )

  // Two-source resolution for the session id passed to acp_connect:
  //   1. detail.summary.external_id — DB value, available for tabs opened
  //      from the sidebar (effectiveConversationId equals the real cid).
  //   2. runtimeExternalId — populated by the connSessionId effect
  //      below when SessionStarted fires. This is the ONLY source for tabs
  //      that started as a new conversation: their effectiveConversationId
  //      is locked to a virtual negative id (line 186 useState initializer
  //      runs once), useConversationDetail skips fetching for virtual ids,
  //      and detail stays null forever. Without this fallback, every
  //      reconnect on a new-conversation tab passes sessionId=undefined →
  //      backend takes session/new → DB.external_id is overwritten on the
  //      next prompt → original sid orphaned, agent loses prior context.
  const externalId =
    detail?.summary.external_id ?? runtimeExternalId ?? undefined
  // Agents whose connect path needs a client-provided external id wait for
  // detail. Codex restore is conversation-id based and resolves the durable id
  // server-side, so it starts in parallel with paged history loading instead
  // of adding the detail response time ahead of a long session/resume.
  const awaitingHistoricalSessionId = shouldAwaitHistoricalSessionDetail({
    usesPersistedDetailIdentity,
    agentType: selectedAgent,
    detailLoaded: detail != null,
    detailLoading,
    detailError,
  })
  // Install status of the currently selected agent. An agent can be enabled and
  // platform-available yet have no CLI/SDK installed; selecting one can never
  // connect. Rather than firing a doomed (and racy) auto-connect whose only
  // outcome is a transient "not installed" toast, we skip the connect and
  // surface a persistent install prompt instead (see composerBlockedMessage).
  const { agents: acpAgents } = useAcpAgents()
  const selectedAgentNotInstalled = useMemo(() => {
    const info = acpAgents.find((a) => a.agent_type === selectedAgent)
    return (
      info != null && info.enabled && info.available && !info.installed_version
    )
  }, [acpAgents, selectedAgent])
  // Claude Code / Codex install a separate ACP adapter package rather than the
  // vendor CLI, so the generic "not installed" banner reads as wrong to anyone
  // who has `claude`/`codex` in their terminal. Name the adapter instead.
  const selectedAgentIsAcpAdapter = useMemo(
    () =>
      acpAgents.find((a) => a.agent_type === selectedAgent)?.is_acp_adapter ===
      true,
    [acpAgents, selectedAgent]
  )
  const canAutoConnect =
    (hasPersistedConversation || (agentsLoaded && usableAgentCount > 0)) &&
    !awaitingHistoricalSessionId &&
    // Skip the doomed auto-connect for a not-installed agent ONLY in the draft
    // surfaces, where the persistent install banner explains it instead. A
    // persisted conversation keeps its existing connect-and-surface-the-error
    // behavior (its agent can't be swapped from the picker anyway).
    !(selectedAgentNotInstalled && !hasPersistedConversation) &&
    !(hasPersistedConversation && detailError) &&
    !(hasPersistedConversation && acpLoadError)
  // Draft composer text is keyed PER TAB while unbound: each split group has its
  // own draft, and a single shared key made them overwrite each other. The key
  // survives restarts with the tab id (persisted in the group blob).
  const draftStorageKey = useMemo(() => {
    if (dbConversationId != null) {
      return buildConversationDraftStorageKey(dbConversationId)
    }
    return buildNewConversationDraftStorageKey(tabId)
  }, [dbConversationId, tabId])
  // One-shot handover of the pre-per-tab shared draft, so an in-flight draft
  // isn't stranded by the upgrade (no-ops for every later draft tab).
  useEffect(() => {
    if (dbConversationId != null) return
    adoptLegacyNewConversationDraft(draftStorageKey)
  }, [dbConversationId, draftStorageKey])
  // Use the per-tab workingDir (derived from the tab's own folderId by the
  // parent) rather than the active folder's path — otherwise switching tabs
  // briefly exposes the previous folder's path to the ACP auto-connect
  // effect, and the connection sticks with the wrong cwd.
  const workingDirForConnection = workingDir ?? folder?.path

  const {
    conn,
    modeLoading,
    configOptionsLoading,
    selectorsLoading,
    restorePending,
    autoConnectError,
    handleFocus,
    handleSend: lifecycleSend,
    handleSetConfigOption,
    isCancelling,
    handleCancel,
    handleRespondPermission,
  } = useConnectionLifecycle({
    contextKey: tabId,
    agentType: selectedAgent,
    isActive: isActive && canAutoConnect,
    workingDir: workingDirForConnection,
    sessionId:
      dbConversationId != null && selectedAgent !== "cline"
        ? externalId
        : undefined,
    // Drives cross-client viewer discovery: when another client is already
    // live on this conversation, attach to its connection instead of spawning.
    conversationId: dbConversationId ?? undefined,
    // A cross-group move / unsplit reparents this view (React remounts it)
    // while the tab stays open — that unmount must not tear the connection
    // down. See `isReparentUnmount` for why "still open" alone is too broad.
    isTransientUnmount: useCallback(
      () => isReparentUnmount(useTabStore.getState(), tabId, groupId),
      [tabId, groupId]
    ),
  })
  const { status: connStatus, sessionId: connSessionId } = conn
  // Persist the queue under the STABLE virtual conversation id so undelivered
  // messages survive a reload during a network outage.
  const messageQueue = useMessageQueue(effectiveConversationId, {
    // Remote/web images already live in the server upload jail. Persist only
    // their URI (not duplicate base64) so a reload-safe queue remains small.
    compactUploadedImages: !isLocalDesktop(),
  })
  // Transport link health (web mode). Desktop/remote snapshots are always
  // "connected", so the offline gates below are inert outside web mode.
  const webConnState = useSyncExternalStore(
    subscribeWebConnection,
    getWebConnectionSnapshot,
    getWebConnectionServerSnapshot
  )
  const {
    queue: msgQueue,
    enqueue: mqEnqueue,
    requeueFront: mqRequeueFront,
    getQueueLength: mqGetQueueLength,
    peekNext: mqPeekNext,
    remove: mqRemove,
    markState: mqMarkState,
    retry: mqRetry,
    convertGuideToPrompt: mqConvertGuideToPrompt,
    reorder: mqReorder,
    updateItem: mqUpdateItem,
    editingItemId: mqEditingItemId,
    startEditing: mqStartEditing,
    cancelEditing: mqCancelEditing,
  } = messageQueue
  const connStatusRef = useRef(connStatus)
  useEffect(() => {
    connStatusRef.current = connStatus
  }, [connStatus])
  const isViewerRef = useRef(conn.isViewer)
  useEffect(() => {
    isViewerRef.current = conn.isViewer
  }, [conn.isViewer])
  const isConnecting = connStatus === "connecting" || restorePending
  // The tab's connection is keyed by a stable tabId, but agent switching is
  // async — and for a not-installed target, connect()'s preflight throws BEFORE
  // it tears down the old connection. So `conn` can still describe the PREVIOUS
  // agent while `selectedAgent` has already advanced. When that's the case we
  // must NOT surface the previous agent's selectors / ready-state as the
  // selected one's: doing so showed the old agent's model + config list and
  // (worse) let a send reach the wrong agent. Reconcile everything the composer
  // reads against `selectedAgent`, falling back to that agent's own cached
  // selectors (empty until it connects).
  const connIsForOtherAgent =
    conn.agentType != null && conn.agentType !== selectedAgent
  const effectiveModes = connIsForOtherAgent
    ? (getCachedSelectors(selectedAgent)?.modes ?? null)
    : conn.modes
  const effectiveConfigOptions = connIsForOtherAgent
    ? (getCachedSelectors(selectedAgent)?.configOptions ?? null)
    : conn.configOptions
  // The live connection is ready for THIS tab only when it's connected AND its
  // cwd matches the tab's intended working dir. A just-retargeted chat draft (or
  // any mid-reconnect) can briefly read a stale "connected" for the PREVIOUS cwd;
  // sending then would deliver the prompt to the wrong agent/workspace. Every
  // direct send gates on this (handleSend), mirroring the flush effect's guard.
  // No-op for normal conversations, whose connected cwd always equals intended.
  // A connection still bound to a different agent is never "ready" for the
  // selected one — it would otherwise let a send reach the previous agent.
  const restoredIdentityReady =
    !hasPersistedConversation ||
    !externalId ||
    (conn.sessionId === externalId &&
      conn.conversationId === dbConversationId &&
      (selectedAgent !== "codex" || conn.codegMcpAvailable))
  const connectionReady =
    !connIsForOtherAgent &&
    restoredIdentityReady &&
    conn.promptReady &&
    isConnectionReady(
      connStatus,
      conn.connectedWorkingDir,
      workingDirForConnection
    )
  const guideConnectionReady =
    !connIsForOtherAgent &&
    restoredIdentityReady &&
    conn.promptReady &&
    connStatus === "prompting" &&
    (conn.connectedWorkingDir ?? null) === (workingDirForConnection ?? null)
  // Present "connecting" to the composer while connected-but-not-ready, so it
  // disables its send affordance instead of inviting a submit handleSend rejects.
  // While the live connection still belongs to a different agent, present the
  // selected agent's real state: "disconnected" when it isn't installed (the
  // install banner explains why), otherwise "connecting" (the switch is in
  // flight). Only ever differs from connStatus during those transient windows.
  const composerConnStatus = connIsForOtherAgent
    ? selectedAgentNotInstalled
      ? "disconnected"
      : "connecting"
    : restorePending || (connStatus === "connected" && !connectionReady)
      ? "connecting"
      : connStatus
  const connectionModes = useMemo(
    () => effectiveModes?.available_modes ?? [],
    [effectiveModes]
  )
  const connectionConfigOptions = useMemo(
    () => effectiveConfigOptions ?? [],
    [effectiveConfigOptions]
  )
  const connectionCommands = useMemo(
    () => (connIsForOtherAgent ? [] : (conn.availableCommands ?? [])),
    [connIsForOtherAgent, conn.availableCommands]
  )
  const selectedModeId = useMemo(() => {
    if (connectionModes.length === 0) return null
    if (modeId && connectionModes.some((mode) => mode.id === modeId)) {
      return modeId
    }
    return effectiveModes?.current_mode_id ?? connectionModes[0]?.id ?? null
  }, [effectiveModes, connectionModes, modeId])

  // The single blocking message shown in the composer's inline banner (clicking
  // it opens Agent Settings). The not-installed prompt takes priority: it's the
  // actionable one and, unlike the connect-time toast, it's deterministic — it
  // appears the moment a not-installed agent is selected, independent of whether
  // a (deduped/superseded) connect attempt ever reached the preflight.
  const composerBlockedMessage = selectedAgentNotInstalled
    ? tWelcome(
        selectedAgentIsAcpAdapter
          ? "agentAdapterNotInstalled"
          : "agentNotInstalled",
        { agent: getAgentLabel(selectedAgent) }
      )
    : (autoConnectError ?? agentConnectError)

  // A queued restore-wait should not look eternal when the authoritative ACP
  // connect attempt has failed. Keep the draft, but expose an explicit Retry
  // action on that queue item; retry also wakes the existing lifecycle connect
  // path rather than requiring the user to retype or hunt for Reload.
  useEffect(() => {
    if (!autoConnectError) return
    for (const item of msgQueue) {
      if (item.state === "waiting_session_restore") {
        mqMarkState(item.id, "failed", autoConnectError)
      }
    }
  }, [autoConnectError, mqMarkState, msgQueue])

  useEffect(() => {
    if (connSessionId) {
      sessionIdRef.current = connSessionId
    }
  }, [connSessionId])

  useEffect(() => {
    if (connStatus !== "prompting") return
    const clientMessageId = getRuntimeSession(
      effectiveConversationId
    )?.activeTurnToken
    if (!clientMessageId) return
    setPromptDeliveryPhase(effectiveConversationId, clientMessageId, "running")
  }, [connStatus, effectiveConversationId, setPromptDeliveryPhase])

  // Mirror the connection's load failure (set on `session_load_failed` from
  // the agent) onto the per-conversation runtime session so the detail UI
  // can surface it next to detail-load errors. Cleared automatically when
  // the connection's loadError clears (e.g. via Reload).
  const connLoadError = conn.loadError
  useEffect(() => {
    setAcpLoadError(effectiveConversationId, connLoadError ?? null)
  }, [connLoadError, effectiveConversationId, setAcpLoadError])

  // Promote the completed turn on the prompting→idle edge. (There is no longer
  // an ordering constraint against a setLiveMessage cleanup: the liveMessage
  // sink writes the runtime store from the connection dispatch, not a React
  // effect — see registerLiveMessageSink.)
  const prevConnStatusRef = useRef(connStatus)
  useEffect(() => {
    const wasPrompting = prevConnStatusRef.current === "prompting"
    prevConnStatusRef.current = connStatus
    if (!wasPrompting || connStatus === "prompting") return

    const completedClientMessageId =
      getRuntimeSession(effectiveConversationId)?.activeTurnToken ?? null
    // Turn completed — promote liveMessage + optimisticTurns to localTurns.
    // Don't pass conn.liveMessage: this panel no longer subscribes to it (the
    // connection snapshot is stable across streaming tokens — see useConnection),
    // so reading it here would be stale. COMPLETE_TURN falls back to
    // session.liveMessage, which the connection dispatch's sink wrote
    // synchronously as the final chunk landed (turn_complete flushes the stream
    // queue BEFORE the status change), so it already holds the final message.
    completeTurn(effectiveConversationId)
    promptSubmitPendingRef.current = false

    // Cancel a previous completion reconciliation (handles rapid consecutive
    // turns), then poll until the transcript contains the final assistant
    // record. This heals dropped stream deltas without letting an immediate
    // pre-flush read erase the correct in-memory reply.
    syncCancelRef.current?.()
    syncCancelRef.current = null

    const persistedId = dbConvIdRef.current
    if (persistedId && persistedId > 0) {
      syncCancelRef.current = reconcileCompletedTurn(
        persistedId,
        effectiveConversationId,
        completedClientMessageId,
        () => setSendSignal((previous) => previous + 1)
      )
    }
  }, [
    completeTurn,
    connStatus,
    effectiveConversationId,
    reconcileCompletedTurn,
  ])

  // Auto-send queued messages when agent finishes responding.
  // Refs are synced via useEffect; the auto-send effect is declared
  // AFTER completeTurn so React runs it second.
  const autoSendQueueRef =
    useRef<(intent: "prompt" | "guide") => QueuedMessage | undefined>(
      mqPeekNext
    )
  useEffect(() => {
    autoSendQueueRef.current = mqPeekNext
  }, [mqPeekNext])
  const handleSendRef = useRef<
    (
      draft: PromptDraft,
      modeId?: string | null,
      opts?: {
        fromQueueFlush?: boolean
        clientMessageId?: string
        queueItemId?: string
      }
    ) => void
  >(() => {})
  // A queued item stays persisted until `/acp_prompt` confirms acceptance.
  // This ref single-flights that retained head while React effects re-run.
  const queueSendInFlightRef = useRef<string | null>(null)
  const connectionReadyRef = useRef(connectionReady)
  useEffect(() => {
    connectionReadyRef.current = connectionReady
  }, [connectionReady])
  // A response can be lost after the Server accepted a prompt. On reload the
  // persistent queue still contains that id, while conversation detail carries
  // the durable acceptance receipt. Settle it locally before auto-flush so a
  // status-recovery request cannot create a stray optimistic bubble after the
  // original turn has already completed.
  useEffect(() => {
    const acceptedIds = new Set(
      (detail?.artifact_runs ?? [])
        .filter((run) => Boolean(run.prompt_accepted_at))
        .map((run) => run.client_message_id)
        .filter((id): id is string => Boolean(id))
    )
    if (acceptedIds.size === 0) return
    for (const item of msgQueue) {
      if (item.intent === "prompt" && acceptedIds.has(item.clientMessageId)) {
        mqRemove(item.id)
        setPromptDeliveryPhase(
          effectiveConversationId,
          item.clientMessageId,
          "accepted"
        )
      }
    }
  }, [
    detail?.artifact_runs,
    effectiveConversationId,
    mqRemove,
    msgQueue,
    setPromptDeliveryPhase,
  ])
  // Timestamp of the last send that bounced with TurnBusyError. The flush below
  // backs off after a bounce so repeated busy rejections (backend still running
  // another turn while this client believes it is idle) don't spin one failed
  // send per round-trip.
  const lastFlushBounceAtRef = useRef(0)

  // Flush queued messages whenever the agent is idle. This is the queue's send
  // engine, covering BOTH:
  //   - the normal case: a message queued while the agent was prompting, sent
  //     once the turn completes (prompting→connected drives syncState→idle); and
  //   - a draft re-queued by a bounced concurrent send that landed AFTER the
  //     prompting→connected transition already passed — which an edge-triggered
  //     flush would strand until the next turn.
  // Gated on syncState !== "awaiting_persist" so exactly one item flushes at a
  // time: dequeuing + sending appends an optimistic turn → awaiting_persist,
  // which blocks re-entry until that send settles (the turn completes, or it
  // bounces and rolls back to idle to retry the next item). A bounce backoff
  // rate-limits retries against a still-busy backend.
  useEffect(() => {
    if (!connectionReady) return
    // Transport link down (web mode): the ACP connStatus can read a stale
    // "connected" during a WS blip. Don't flush onto a reconnecting link — the
    // send would fail and re-queue in a tight loop. Resumes automatically when
    // the link recovers (this effect re-runs on webConnState change).
    if (webConnState !== "connected") return
    // Don't flush onto a connection whose cwd doesn't match the tab's intended
    // working dir. This matters for a just-bound chat conversation: bind switches
    // the tab's workingDir from the draft's previous folder to the scratch dir,
    // and for one render `connStatus` can still read the stale "connected" of the
    // old-folder session before the reconnect lands. Flushing then would deliver
    // the queued prompt to the wrong folder's agent. (No-op for normal
    // conversations, whose connection cwd always equals the intended one.)
    if (runtimeSyncState === "awaiting_persist") return
    if (msgQueue[0]?.intent !== "prompt" || !mqPeekNext("prompt")) return
    // setTimeout (not microtask) so a COMPLETE_TURN commit settles first AND so
    // a just-bounced retry waits out the backoff window before re-sending.
    const wait = flushRetryDelayMs(Date.now(), lastFlushBounceAtRef.current)
    const timer = setTimeout(() => {
      if (!connectionReadyRef.current) return
      if (queueSendInFlightRef.current) return
      const next = autoSendQueueRef.current("prompt")
      if (next) {
        // Keep the item in persistent storage until the backend acknowledges
        // this exact client id. A reload can safely retry the retained item;
        // backend idempotency turns a lost response into a status recovery.
        queueSendInFlightRef.current = next.id
        handleSendRef.current(next.draft, next.modeId, {
          fromQueueFlush: true,
          clientMessageId: next.clientMessageId,
          queueItemId: next.id,
        })
      }
    }, wait)
    return () => clearTimeout(timer)
  }, [connectionReady, webConnState, runtimeSyncState, msgQueue, mqPeekNext])

  // Mirror the connection's liveMessage into the runtime session OUTSIDE React.
  // The connection dispatch invokes this sink synchronously whenever liveMessage
  // changes (streaming deltas, tool updates, the prompt-start reset), so the
  // streaming content flows straight to the runtime store — which the message
  // list renders — WITHOUT this keep-alive panel re-rendering per token (the old
  // mirror effect required a per-token render just to run). The sink writes
  // non-null values with isLive = (status === "prompting"), which tells the
  // runtime reducer to bypass its stale-reconnect-replay guard (matters for the
  // rekey path: close+reopen mid-turn, where detail.turns may already hold user
  // turns that would otherwise drop the live assistant stream). Turn-end clearing
  // is owned by COMPLETE_TURN (nulls liveMessage); unmount clearing by
  // removeConversation. `tabId` is the connection contextKey.
  useEffect(() => {
    return acpActions.registerLiveMessageSink(tabId, (liveMessage, isLive) =>
      setLiveMessage(effectiveConversationId, liveMessage, isLive)
    )
  }, [acpActions, tabId, effectiveConversationId, setLiveMessage])

  // Cross-client VIEWER (Bug 2): mirror the connection's in-flight user prompt
  // (from a snapshot's `pending_user_message`, captured when we attach
  // mid-turn) into the runtime as a synthesized user turn. The reducer
  // sender-guards + dedups by id, so this is a no-op on the sender and
  // idempotent against the live `user_message` event below. This branch covers
  // the prompt that was sent BEFORE we attached; the live handler covers
  // prompts sent AFTER.
  useEffect(() => {
    const pending = conn.pendingUserMessage
    if (!pending) return
    appendViewerUserTurn(
      effectiveConversationId,
      buildUserTurnFromMessageBlocks(pending.messageId, pending.blocks)
    )
  }, [conn.pendingUserMessage, effectiveConversationId, appendViewerUserTurn])

  // Native guide messages are turn-scoped state, so the snapshot path restores
  // them after a browser refresh/reconnect. Exact message ids dedup the sending
  // client's optimistic bubble against this authoritative echo.
  useEffect(() => {
    for (const message of conn.steerMessages) {
      appendViewerUserTurn(
        effectiveConversationId,
        buildUserTurnFromMessageBlocks(message.messageId, message.blocks)
      )
    }
  }, [conn.steerMessages, effectiveConversationId, appendViewerUserTurn])

  // Cross-client VIEWER (Bug 2): a `user_message` event for THIS connection
  // that arrives while we're attached. The owner added its user turn
  // optimistically; a viewer only receives the assistant stream, so without
  // this the reply would render with no user message above it. Sender-guarded +
  // idempotent in the reducer (the sender's own echo is a no-op).
  useAcpEvent(
    useCallback(
      (envelope: EventEnvelope) => {
        if (envelope.type !== "user_message") return
        if (envelope.connection_id !== conn.connectionId) return
        appendViewerUserTurn(
          effectiveConversationId,
          buildUserTurnFromMessageBlocks(envelope.message_id, envelope.blocks)
        )
      },
      [conn.connectionId, effectiveConversationId, appendViewerUserTurn]
    )
  )

  useEffect(() => {
    if (effectiveConversationId <= 0) return
    // Only ever WRITE a real id — never clear one. `detail` is null while a
    // (re)fetch is in flight, and writing null then would wipe a session id the
    // connSessionId effect below had already resolved. That store value is one
    // of the two sources `externalId` (and therefore the sessionId passed to
    // acp_connect) resolves from, so clearing it can turn a reconnect into
    // session/new and strand the conversation's history. Nothing ever nulls a
    // row's external_id, and switching conversations changes the store key
    // rather than clearing this one, so there is no case that needs the clear.
    const persisted = detail?.summary.external_id
    if (!persisted) return
    setExternalId(effectiveConversationId, persisted)
  }, [effectiveConversationId, detail?.summary.external_id, setExternalId])

  useEffect(() => {
    if (!connSessionId) return
    setExternalId(effectiveConversationId, connSessionId)
  }, [connSessionId, effectiveConversationId, setExternalId])

  useEffect(() => {
    if (dbConversationId == null) return
    if (reloadSignal === latestReloadSignal.current) return
    latestReloadSignal.current = reloadSignal
    pendingReloadState.current = {
      signal: reloadSignal,
      sawLoading: false,
    }
    refetchDetail(dbConversationId)
  }, [dbConversationId, reloadSignal, refetchDetail])

  useEffect(() => {
    const pending = pendingReloadState.current
    if (!pending) return

    if (detailLoading) {
      pending.sawLoading = true
      return
    }

    if (!pending.sawLoading) return

    pendingReloadState.current = null

    if (detailError) {
      toast.error(t("reloadFailed", { message: detailError }))
      return
    }

    toast.success(t("reloaded"))
  }, [detailLoading, detailError, t])

  // Cleanup runtime data on unmount (tab close)
  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      syncCancelRef.current?.()
      if (connStatusRef.current === "prompting" && !isViewerRef.current) {
        // Owner, agent still responding — keep the session for deferred cleanup
        // (the background turn_complete handler removes it once done).
        setPendingCleanup(effectiveConversationId, true)
      } else {
        // Idle owner, or a VIEWER (any status): remove immediately. A viewer's
        // unmount detaches its attach subscription, so no turn_complete will
        // arrive to resolve a deferred cleanup — deferring would leak the
        // runtime session (especially in web mode, which has no event firehose
        // after detach).
        removeConversation(effectiveConversationId)
      }
    }
  }, [effectiveConversationId, removeConversation, setPendingCleanup])

  const handleSend = useCallback(
    (
      draft: PromptDraft,
      selectedModeIdArg?: string | null,
      // `fromQueueFlush` marks the auto-flush draining the queue head — that
      // path always sends and, on a bounce, re-queues at the FRONT. A direct
      // input send (no flag) must NOT jump ahead of already-queued items: when
      // a queue exists it tail-enqueues instead of sending, and on a bounce it
      // re-queues at the TAIL.
      opts?: {
        fromQueueFlush?: boolean
        clientMessageId?: string
        queueItemId?: string
      }
    ) => {
      // Capture the tab's chat-draft state + eager scratch dir synchronously,
      // before any await. A folderless chat draft is NOT special-cased here:
      // its first send takes the exact same gated, inline path as a normal new
      // conversation (the new-tab branch below just creates the row via
      // createChatConversation, reusing this eager dir). The composer is gated
      // on `connected` for chat drafts too, so by the time we get here the agent
      // is live and the prompt is delivered inline — never parked in the queue.
      const sendOwnTab = ownTab
      const fromQueueFlush = opts?.fromQueueFlush ?? false
      const queueItemId = opts?.queueItemId ?? null
      const clientMessageId =
        opts?.clientMessageId ?? `optimistic-${randomUUID()}`

      if (!hasPersistedConversation && !canAutoConnect) {
        setAgentConnectError(tWelcome("enableAgentFirstPlaceholder"))
        return
      }
      // Historical conversations accept input while their exact ACP identity /
      // attach snapshot is still restoring. Retain the whole draft under its
      // stable client id; the queue flushes only after `connectionReady` turns
      // true. A queue item already being retried is updated in place rather
      // than duplicated.
      if (!connectionReady) {
        if (hasPersistedConversation) {
          if (queueItemId) {
            mqMarkState(queueItemId, "waiting_session_restore")
          } else {
            mqEnqueue(draft, selectedModeIdArg ?? null, clientMessageId, {
              state: "waiting_session_restore",
            })
          }
        }
        if (queueItemId) queueSendInFlightRef.current = null
        return
      }

      // Transport link down (web mode): the composer's ACP gate can read a stale
      // "connected" during a WS blip. Park a direct send in the (persisted)
      // queue instead of firing a request that will fail; auto-flush resends it
      // when the link recovers. Inert on desktop/remote. Queue-flush sends are
      // already gated by the flush effect's webConnState check.
      if (!fromQueueFlush && getWebConnectionSnapshot() === "reconnecting") {
        mqEnqueue(draft, selectedModeIdArg ?? null, clientMessageId, {
          state: "waiting_connection",
        })
        return
      }
      // Preserve FIFO: a direct send issued while the queue is non-empty joins
      // the tail rather than racing ahead of the queued items. Read the
      // queue length synchronously (it reflects a same-tick bounce requeue).
      if (shouldQueueDirectSend(fromQueueFlush, mqGetQueueLength())) {
        mqEnqueue(draft, selectedModeIdArg ?? null, clientMessageId)
        return
      }

      // Single-flight every ordinary prompt until the backend has answered the
      // submission request. This closes the double Enter/click window for an
      // existing conversation as well as a new tab.
      if (promptSubmitPendingRef.current) {
        if (queueItemId) queueSendInFlightRef.current = null
        return
      }

      // Keep the existing unbound-create guard too: conversation creation can
      // outlive the submission guard and must never run twice.
      if (
        shouldRejectDuplicateCreate(
          dbConvIdRef.current != null,
          createConversationPendingRef.current
        )
      ) {
        return
      }
      promptSubmitPendingRef.current = true

      const optimisticTurn = buildOptimisticUserTurnFromDraft(
        draft,
        sharedT("attachedResources"),
        clientMessageId
      )
      appendOptimisticTurn(
        effectiveConversationId,
        optimisticTurn,
        optimisticTurn.id
      )
      setSendSignal((prev) => prev + 1)
      setSyncState(effectiveConversationId, "awaiting_persist")
      setHasSentMessage(true)

      // Backend rejected the send because a turn was already in flight (another
      // co-controlling client, or a "prompting" status this client hadn't
      // observed yet). Roll back the optimistic user turn and drop the draft
      // into the queue above the input box — it auto-sends when the current
      // turn completes, identical to enqueuing while already prompting. Stamp
      // the bounce so the flush backs off instead of immediately retrying.
      let accepted = false
      const onTurnInProgress = () => {
        promptSubmitPendingRef.current = false
        const shouldQueue = shouldQueuePromptFailure({
          backendBusy: true,
          accepted,
          activeClientMessageId: conn.pendingUserMessage?.messageId ?? null,
          clientMessageId,
        })
        if (!shouldQueue) {
          if (queueItemId) mqRemove(queueItemId)
          queueSendInFlightRef.current = null
          setPromptDeliveryPhase(
            effectiveConversationId,
            clientMessageId,
            "running"
          )
          return
        }
        lastFlushBounceAtRef.current = Date.now()
        removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
        // The optimistic send moved the runtime into `awaiting_persist`. This
        // prompt was NOT accepted, so no turn_complete can ever reset it. Put
        // the runtime back at idle before retaining the queue head; otherwise
        // the auto-flush effect permanently refuses to retry this message even
        // after the backend becomes ready.
        setSyncState(effectiveConversationId, "idle")
        setPromptDeliveryPhase(
          effectiveConversationId,
          clientMessageId,
          "queued"
        )
        if (queueItemId) {
          // The retained queue head was never removed, so a Busy response only
          // re-arms it; no dequeue/requeue window and no duplicate item.
          mqMarkState(queueItemId, "queued")
          queueSendInFlightRef.current = null
        } else if (fromQueueFlush) {
          // Backward-compatible fallback for callers that mark a flush without
          // passing the retained queue id.
          mqRequeueFront(draft, selectedModeIdArg ?? null, clientMessageId)
        } else {
          mqEnqueue(draft, selectedModeIdArg ?? null, clientMessageId)
        }
      }
      const onAccepted = () => {
        accepted = true
        promptSubmitPendingRef.current = false
        if (queueItemId) mqRemove(queueItemId)
        queueSendInFlightRef.current = null
        setPromptDeliveryPhase(
          effectiveConversationId,
          clientMessageId,
          "accepted"
        )
      }
      const onSessionRestorePending = () => {
        promptSubmitPendingRef.current = false
        removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
        setPromptDeliveryPhase(
          effectiveConversationId,
          clientMessageId,
          "queued"
        )
        if (queueItemId) {
          mqMarkState(queueItemId, "waiting_session_restore")
        } else {
          mqEnqueue(draft, selectedModeIdArg ?? null, clientMessageId, {
            state: "waiting_session_restore",
          })
        }
        queueSendInFlightRef.current = null
        setSyncState(effectiveConversationId, "idle")
      }
      const onSendFailed = (error: unknown, ambiguous: boolean) => {
        promptSubmitPendingRef.current = false
        setPromptDeliveryPhase(
          effectiveConversationId,
          clientMessageId,
          "failed",
          ambiguous ? t("promptAcceptanceUnknown") : toErrorMessage(error)
        )
        // A transport loss is ambiguous: the backend may already be running
        // this exact id. Keep that optimistic message for reconnect/detail
        // reconciliation; deterministic failures roll it back immediately.
        if (!ambiguous) {
          removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
        }
        if (queueItemId) {
          mqMarkState(
            queueItemId,
            ambiguous ? "waiting_connection" : "failed",
            toErrorMessage(error)
          )
        } else if (ambiguous) {
          // The HTTP response may be the only thing that was lost. Retain the
          // same id for explicit retry; backend/client reconciliation dedups an
          // already-accepted request.
          mqEnqueue(draft, selectedModeIdArg ?? null, clientMessageId, {
            state: "waiting_connection",
            error: toErrorMessage(error),
          })
        }
        queueSendInFlightRef.current = null
        setSyncState(effectiveConversationId, "idle")
      }

      // Pin the tab if it was a temporary preview (single-click opened)
      if (ownTab && !ownTab.isPinned) {
        pinTab(tabId)
      }

      const persistedId = dbConvIdRef.current
      if (persistedId) {
        // Existing-tab path: row already exists, send immediately with the
        // conversation_id pinned so the backend reuses our row instead of
        // creating a duplicate.
        lifecycleSend(draft, selectedModeIdArg, {
          folderId,
          conversationId: persistedId,
          // The backend echoes this as the broadcast UserMessage's message_id,
          // so viewers' synthesized user turn dedups against our own optimistic
          // turn by exact id (and never suppresses a different sender's prompt).
          clientMessageId: optimisticTurn.id,
          onTurnInProgress,
          onSessionRestorePending,
          onAccepted,
          onSendFailed,
        })
        return
      }

      // New-tab path: create the DB row first, then send with the new id
      // pinned. This prevents the backend's send_prompt_linked from racing
      // us to create its own conversation row. A folderless chat draft creates
      // via createChatConversation (reusing the eager scratch dir) and binds to
      // its hidden chat folder; every other step — the optimistic turn
      // appended above, the inline lifecycleSend, the rollback — is identical to
      // a normal new conversation. This is the whole point of the fix: after the
      // scratch dir exists, chat mode shares the normal send path and never
      // depends on the flush-on-connect queue to deliver its first prompt.
      if (createConversationPendingRef.current) return
      createConversationPendingRef.current = true
      const title = getPromptDraftDisplayText(
        draft,
        sharedT("attachedResources")
      ).slice(0, 80)
      const chatSend = sendOwnTab?.isChat === true
      const chatExistingDir = sendOwnTab?.workingDir

      void (async () => {
        try {
          let newConversationId: number
          // The send's folderId defaults to the active folder; a chat send
          // overrides it with the backend-created hidden chat folder.
          let sendFolderId = folderId
          if (chatSend) {
            const res = await createChatConversation(
              selectedAgent,
              title,
              chatExistingDir
            )
            newConversationId = res.conversationId
            sendFolderId = res.folderId
            dbConvIdRef.current = newConversationId
            setExternalId(effectiveConversationId, sessionIdRef.current ?? null)
            // Bind the DB id BEFORE the prompt goes out. The mirror effect
            // below also binds, but only after a re-render — this closes that
            // window and covers the unmounted-early return just under it.
            setDbConversationId(effectiveConversationId, newConversationId)
            if (!mountedRef.current) {
              setPendingCleanup(effectiveConversationId, true)
              refreshConversations()
              return
            }
            // Seed allFolders with the hidden chat folder so the tab's new
            // folderId resolves (cwd / active-folder) on the next render. bind
            // reuses the eager scratch dir as workingDir, so the connection's
            // cwd does not move and no reconnect is triggered.
            upsertFolder(res.folder)
            setCreatedConversationId(newConversationId)
            bindConversationTab(
              tabId,
              newConversationId,
              selectedAgent,
              title,
              effectiveConversationId,
              res.folderId,
              res.folder.path
            )
          } else {
            newConversationId = await createConversation(
              folderId,
              selectedAgent,
              title
            )
            dbConvIdRef.current = newConversationId
            // Set external ID on the stable virtual session (no migration needed —
            // effectiveConversationId never changes, so the session stays in place).
            // DB persistence of external_id is now backend-driven from
            // send_prompt_linked once the row is linked, so no explicit DB write here.
            setExternalId(effectiveConversationId, sessionIdRef.current ?? null)
            // Bind the DB id BEFORE the prompt goes out (see the chat branch).
            setDbConversationId(effectiveConversationId, newConversationId)
            if (!mountedRef.current) {
              // Component unmounted while creating — mark for deferred cleanup
              // so the background turn_complete handler can clean up later.
              setPendingCleanup(effectiveConversationId, true)
              refreshConversations()
              return
            }
            setCreatedConversationId(newConversationId)
            bindConversationTab(
              tabId,
              newConversationId,
              selectedAgent,
              title,
              effectiveConversationId
            )
          }
          clearMessageInputDraft(buildNewConversationDraftStorageKey(tabId))
          refreshConversations()

          // Now that the row exists, kick off the actual prompt with the
          // conversation_id pinned so the backend adopts our row instead of
          // creating a duplicate one.
          lifecycleSend(draft, selectedModeIdArg, {
            folderId: sendFolderId,
            conversationId: newConversationId,
            clientMessageId: optimisticTurn.id,
            onTurnInProgress,
            onSessionRestorePending,
            onAccepted,
            onSendFailed,
          })
        } catch (e) {
          promptSubmitPendingRef.current = false
          console.error("[ConversationTabView] create conversation:", e)
          // A failed create (chat OR normal) must fully restore the pre-send
          // state, not strand the user behind a blank panel:
          //   1. drop the optimistic turn (no ghost stuck in awaiting_persist),
          //   2. return syncState to idle,
          //   3. setHasSentMessage(false) → re-enters welcome mode (otherwise the
          //      welcome screen never returns and the list is empty),
          //   4. re-seed the draft text — message-input clears it synchronously on
          //      send, so without this the user's prompt is lost on failure,
          //   5. surface the error on the welcome banner so it isn't silent.
          removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
          setPromptDeliveryPhase(
            effectiveConversationId,
            clientMessageId,
            "failed",
            toErrorMessage(e)
          )
          setSyncState(effectiveConversationId, "idle")
          setHasSentMessage(false)
          const draftText = draft.displayText.trim()
          if (draftText) {
            saveMessageInputDraft(
              buildNewConversationDraftStorageKey(tabId),
              draftText
            )
          }
          if (mountedRef.current) {
            setAgentConnectError(tWelcome("createConversationFailed"))
          }
        } finally {
          createConversationPendingRef.current = false
        }
      })()
    },
    [
      appendOptimisticTurn,
      removeOptimisticTurn,
      mqEnqueue,
      mqMarkState,
      mqRemove,
      mqRequeueFront,
      mqGetQueueLength,
      bindConversationTab,
      canAutoConnect,
      connectionReady,
      conn.pendingUserMessage,
      effectiveConversationId,
      folderId,
      hasPersistedConversation,
      lifecycleSend,
      pinTab,
      refreshConversations,
      selectedAgent,
      setDbConversationId,
      setExternalId,
      setPendingCleanup,
      setPromptDeliveryPhase,
      setSyncState,
      sharedT,
      ownTab,
      tWelcome,
      t,
      tabId,
      upsertFolder,
    ]
  )

  // Sync handleSend ref for auto-send effect (declared before handleSend)
  useEffect(() => {
    handleSendRef.current = handleSend
  }, [handleSend])

  const handleForkSend = useCallback(
    (draft: PromptDraft, selectedModeIdArg?: string | null) => {
      mqEnqueue(
        draft,
        selectedModeIdArg ?? null,
        `optimistic-${randomUUID()}`,
        { intent: "fork" }
      )
    },
    [mqEnqueue]
  )

  useEffect(() => {
    const queueBranch = (rawEvent: Event) => {
      const event = rawEvent as CustomEvent<QueueBranchCreationRequest>
      if (event.detail.conversationId !== dbConversationId) return
      event.preventDefault()
      mqEnqueue(
        { blocks: [], displayText: t("branch.create") },
        event.detail.modeId,
        event.detail.requestId,
        { intent: "branch" }
      )
    }
    window.addEventListener(QUEUE_BRANCH_CREATION_EVENT, queueBranch)
    return () =>
      window.removeEventListener(QUEUE_BRANCH_CREATION_EVENT, queueBranch)
  }, [dbConversationId, mqEnqueue, t])

  const forkSendInFlightRef = useRef<string | null>(null)
  useEffect(() => {
    const item = msgQueue[0]
    if (!item || (item.intent !== "fork" && item.intent !== "branch")) return
    const needsLiveSourceSession = item.intent === "fork"
    const sourceTurnIsActive =
      runtimeSyncState !== "idle" || connStatus === "prompting"
    if (
      forkSendInFlightRef.current ||
      sourceTurnIsActive ||
      (needsLiveSourceSession &&
        (!connectionReady || connStatus !== "connected")) ||
      webConnState !== "connected" ||
      dbConversationId == null
    ) {
      return
    }
    forkSendInFlightRef.current = item.id
    const selectors = getCachedSelectors(selectedAgent)
    void createConversationBranch({
      requestId: item.clientMessageId,
      sourceConversationId: dbConversationId,
      preferredModeId: item.modeId ?? selectors?.modes?.current_mode_id ?? null,
      preferredConfigValues: Object.fromEntries(
        (selectors?.configOptions ?? []).map((option) => [
          option.id,
          String(option.kind.current_value),
        ])
      ),
    })
      .then(async (result) => {
        if (item.intent === "fork") {
          enqueuePersistedMessageForConversation(
            result.branchConversationId,
            { ...item, intent: "prompt", state: "queued", error: null },
            !isLocalDesktop()
          )
        }
        mqRemove(item.id)
        await refreshConversations()
        const branchTitle = useAppWorkspaceStore
          .getState()
          .conversations.find(
            (conversation) => conversation.id === result.branchConversationId
          )?.title
        openTab(
          result.folderId,
          result.branchConversationId,
          selectedAgent,
          true,
          branchTitle ?? `${ownTab?.title ?? "未命名会话"} · 分支`
        )
      })
      .catch((error: unknown) => {
        mqMarkState(item.id, "failed", toErrorMessage(error))
      })
      .finally(() => {
        forkSendInFlightRef.current = null
      })
  }, [
    connectionReady,
    connStatus,
    dbConversationId,
    mqMarkState,
    mqRemove,
    msgQueue,
    openTab,
    ownTab?.title,
    refreshConversations,
    runtimeSyncState,
    selectedAgent,
    webConnState,
  ])

  const handleOpenAgentsSettings = useCallback(() => {
    openSettingsWindow("agents", { agentType: selectedAgent }).catch((err) => {
      console.error(
        "[ConversationTabView] failed to open settings window:",
        err
      )
    })
  }, [selectedAgent])

  // Manual agent switch only updates local draft state. The single source of
  // truth for (dis)connecting is `useConnectionLifecycle`'s auto-connect
  // effect: when `selectedAgent` changes, the hook re-fires `connect()`,
  // which internally disconnects the old agent's connection at the same
  // contextKey before creating the new one (acp-connections-context.tsx).
  // Doing the disconnect+reconnect here too would race the lifecycle path:
  // a late-returning disconnect would dispatch CONNECTION_REMOVED by
  // contextKey and wipe the new connection's frontend state, leaving a
  // backend orphan.
  const handleAgentSelect = useCallback(
    (nextAgentType: AgentType) => {
      if (nextAgentType === selectedAgentRef.current) return
      if (dbConvIdRef.current) return

      setDraftAgentType(nextAgentType)
      setModeId(getSavedModeId(nextAgentType))
      setAgentConnectError(null)
      // Real user click — clear the provisional flag so TabProvider's
      // correction effect leaves this tab alone.
      confirmDraftAgent(tabId, nextAgentType)
    },
    [confirmDraftAgent, tabId]
  )

  // AgentSelector auto-fallback: the requested default agent was missing
  // or unavailable, so it picked a substitute on its own. Sync local UI
  // state (so the connection points at the right agent immediately) but
  // mark the tab as still provisional — TabProvider's correction effect
  // will re-resolve against the folder's saved default once all three
  // hydration gates are open, and overwrite this substitute if needed.
  const handleAgentFallback = useCallback(
    (nextAgentType: AgentType) => {
      if (nextAgentType === selectedAgentRef.current) return
      if (dbConvIdRef.current) return

      setDraftAgentType(nextAgentType)
      setModeId(getSavedModeId(nextAgentType))
      setAgentConnectError(null)
      setDraftAgentFromFallback(tabId, nextAgentType)
    },
    [setDraftAgentFromFallback, tabId]
  )

  const handleModeChange = useCallback(
    (newModeId: string) => {
      setModeId(newModeId)
      // Persist mode selection to localStorage immediately. Use effectiveModes
      // (reconciled to selectedAgent) rather than the raw connection modes, so a
      // mode change made during a cross-agent switch window can't save the
      // previous agent's mode shape under the selected agent.
      if (effectiveModes) {
        saveModePreference(selectedAgent, {
          ...effectiveModes,
          current_mode_id: newModeId,
        })
      }
    },
    [effectiveModes, selectedAgent]
  )

  const handleAnswerQuestion = useCallback(
    (answer: string) => {
      if (connStatus !== "connected") return
      const optimisticTurn: MessageTurn = {
        id: `optimistic-${randomUUID()}`,
        role: "user",
        blocks: [{ type: "text", text: answer }],
        timestamp: new Date().toISOString(),
      }
      const draft: PromptDraft = {
        blocks: [{ type: "text", text: answer }],
        displayText: answer,
      }
      appendOptimisticTurn(
        effectiveConversationId,
        optimisticTurn,
        optimisticTurn.id
      )
      setSendSignal((prev) => prev + 1)
      setSyncState(effectiveConversationId, "awaiting_persist")
      const onTurnInProgress = () => {
        lastFlushBounceAtRef.current = Date.now()
        removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
        setPromptDeliveryPhase(
          effectiveConversationId,
          optimisticTurn.id,
          "queued"
        )
        // A direct answer (never dequeued from the queue) re-queues at the
        // TAIL — it was sent after any already-queued items, so FIFO keeps it
        // behind them. (Only the auto-flush path, whose draft WAS the head,
        // re-queues at the front.)
        mqEnqueue(draft, null, optimisticTurn.id)
      }
      lifecycleSend(draft, null, {
        clientMessageId: optimisticTurn.id,
        onTurnInProgress,
        onAccepted: () =>
          setPromptDeliveryPhase(
            effectiveConversationId,
            optimisticTurn.id,
            "accepted"
          ),
        onSendFailed: (error, ambiguous) => {
          setPromptDeliveryPhase(
            effectiveConversationId,
            optimisticTurn.id,
            "failed",
            ambiguous ? t("promptAcceptanceUnknown") : toErrorMessage(error)
          )
          if (!ambiguous) {
            removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
          }
          setSyncState(effectiveConversationId, "idle")
        },
      })
    },
    [
      appendOptimisticTurn,
      removeOptimisticTurn,
      mqEnqueue,
      connStatus,
      effectiveConversationId,
      lifecycleSend,
      setPromptDeliveryPhase,
      setSyncState,
      t,
    ]
  )

  // Answer a blocking multiple-choice `ask_user_question`. Routes straight to
  // the dedicated answer endpoint (NOT a prompt) so it resolves the parked tool
  // call; the backend broadcasts `question_resolved` to clear the card on every
  // client.
  const handleAnswerAskQuestion = useCallback(
    (questionId: string, answer: QuestionAnswer) =>
      acpActions.answerQuestion(tabId, questionId, answer),
    [acpActions, tabId]
  )

  // Grok `exit_plan_mode` approval: resolve the blocked ext request. The backend
  // broadcasts `plan_approval_resolved` to clear the card on every client.
  //
  // "Request changes" is special. Grok discards the reply `feedback` on the
  // keep-planning path (confirmed against 0.2.111 — only `approved`/`abandoned`
  // consume it), and its own TUI instead delivers the revision notes as a
  // follow-up user turn (`s` moves focus to the prompt). Mirror that: after
  // resolving keep-planning, send the notes as a normal prompt so Grok — still
  // in plan mode — revises and re-presents the plan. The send path queues the
  // prompt if the keep-planning turn is still winding down, then flushes when
  // idle (same optimistic-turn + re-queue dance as `handleAnswerQuestion`).
  const handleAnswerPlanApproval = useCallback(
    (approvalId: string, answer: PlanApprovalAnswer) => {
      const result = acpActions.answerPlanApproval(tabId, approvalId, answer)
      const notes = answer.feedback?.trim()
      if (
        answer.decision === "request_changes" &&
        notes &&
        connStatus === "connected"
      ) {
        const optimisticTurn: MessageTurn = {
          id: `optimistic-${randomUUID()}`,
          role: "user",
          blocks: [{ type: "text", text: notes }],
          timestamp: new Date().toISOString(),
        }
        const draft: PromptDraft = {
          blocks: [{ type: "text", text: notes }],
          displayText: notes,
        }
        appendOptimisticTurn(
          effectiveConversationId,
          optimisticTurn,
          optimisticTurn.id
        )
        setSendSignal((prev) => prev + 1)
        setSyncState(effectiveConversationId, "awaiting_persist")
        lifecycleSend(draft, null, {
          clientMessageId: optimisticTurn.id,
          // Rejected because the keep-planning turn was still in flight — roll
          // back the optimistic turn and re-queue at the tail so it isn't lost.
          onTurnInProgress: () => {
            lastFlushBounceAtRef.current = Date.now()
            removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
            mqEnqueue(draft, null)
          },
          // Any other failure: settle the optimistic state (the lifecycle
          // hook already toasted). No re-queue — deterministic failures loop.
          onSendFailed: () => {
            removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
          },
        })
      }
      return result
    },
    [
      acpActions,
      tabId,
      connStatus,
      appendOptimisticTurn,
      removeOptimisticTurn,
      mqEnqueue,
      effectiveConversationId,
      lifecycleSend,
      setSyncState,
    ]
  )

  // Queue edit flow: derive editing draft text from queue state
  const editingQueueDraftText = useMemo(() => {
    if (!mqEditingItemId) return null
    const item = msgQueue.find((m) => m.id === mqEditingItemId)
    return item?.draft.displayText ?? null
  }, [mqEditingItemId, msgQueue])

  // The editing item's full blocks, so the composer can restore inline badges +
  // attachments (not just the display text) when re-opening a queued message.
  const editingQueueDraftBlocks = useMemo(() => {
    if (!mqEditingItemId) return null
    const item = msgQueue.find((m) => m.id === mqEditingItemId)
    return item?.draft.blocks ?? null
  }, [mqEditingItemId, msgQueue])

  const handleQueueEdit = useCallback(
    (id: string) => {
      mqStartEditing(id)
    },
    [mqStartEditing]
  )

  const handleQueueCancelEdit = useCallback(() => {
    mqCancelEditing()
  }, [mqCancelEditing])

  const handleQueueRetry = useCallback(
    (id: string) => {
      mqRetry(id)
      handleFocus()
    },
    [handleFocus, mqRetry]
  )

  const handleSaveQueueEdit = useCallback(
    (draft: PromptDraft) => {
      if (mqEditingItemId) {
        mqUpdateItem(mqEditingItemId, draft)
      }
    },
    [mqEditingItemId, mqUpdateItem]
  )

  const showDraftHeader = !hasPersistedConversation && !hasSentMessage
  const isWelcomeMode = showDraftHeader

  const handleQuickAction = useCallback((payload: ComposerInjectContent) => {
    setComposerInject(payload)
  }, [])

  const handleComposerInjectConsumed = useCallback(() => {
    setComposerInject(null)
  }, [])

  // Quote a transcript selection into the composer. A fresh object every time so
  // quoting the same passage twice still re-fires the composer's inject effect.
  const handleQuoteSelection = useCallback((selected: string) => {
    const quoted = buildQuotedMarkdown(selected)
    if (!quoted) return
    setComposerInject({ text: quoted, mode: "append" })
  }, [])

  const canShowDetailErrorActions =
    hasPersistedConversation && dbConversationId != null && !!folder
  const handleReloadDetail = useCallback(() => {
    if (dbConversationId == null) return
    // Clear the ACP load failure so canAutoConnect re-enables and the next
    // auto-connect attempt is allowed to retry session/load. The mirror
    // effect above syncs this back into the runtime session as null.
    if (acpLoadError) {
      acpActions.clearAcpLoadError(tabId)
    }
    refetchDetail(dbConversationId)
  }, [acpActions, acpLoadError, dbConversationId, refetchDetail, tabId])
  // Open (or re-activate) the singleton draft tab BEFORE closing the failing
  // tab. closeTab auto-creates a replacement draft when it removes the last
  // tab, and `openNewConversationTab` reads `rawTabsRef.current` which
  // wouldn't yet reflect either pending update if we closed first — the
  // singleton check would miss the replacement and we'd end up with two
  // drafts. Doing it in this order means the second `setTabs` (closeTab)
  // runs against the result of the first.
  const handleOpenNewSession = useCallback(() => {
    if (!folder) return
    // Retry-from-error: user wants a fresh draft in the same conversation
    // context, so inherit the active tab's agent when the folder has no
    // pinned default.
    openNewConversationTab(folder.id, workingDirForConnection ?? folder.path, {
      inheritFromActive: true,
    })
    closeTab(tabId)
  }, [closeTab, folder, openNewConversationTab, tabId, workingDirForConnection])

  // A session/load failure no longer hijacks the whole message area (the
  // transcript stays readable — see message-list-view's blockingLoadError);
  // instead the failure lands here, as a banner docked where the composer
  // sits, carrying the same Reload / New session recovery actions. Persisted
  // conversations only: drafts never session/load.
  const acpLoadErrorBanner =
    hasPersistedConversation && acpLoadError ? (
      <div
        role="alert"
        className="flex w-full items-center gap-2 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive"
      >
        <AlertCircle aria-hidden="true" className="h-4 w-4 shrink-0" />
        <span
          className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap"
          title={acpLoadError}
        >
          {acpLoadError}
        </span>
        {canShowDetailErrorActions && (
          <>
            <button
              type="button"
              onClick={handleReloadDetail}
              disabled={detailLoading}
              aria-busy={detailLoading}
              className="flex shrink-0 items-center gap-1 rounded border border-destructive/40 px-2 py-0.5 font-medium transition-colors hover:bg-destructive/10 disabled:pointer-events-none disabled:opacity-50"
            >
              {detailLoading ? (
                <Loader2 aria-hidden="true" className="h-3 w-3 animate-spin" />
              ) : (
                <RefreshCw aria-hidden="true" className="h-3 w-3" />
              )}
              {tMessageList("errorActionReload")}
            </button>
            <button
              type="button"
              onClick={handleOpenNewSession}
              className="flex shrink-0 items-center gap-1 rounded border border-destructive/40 px-2 py-0.5 font-medium transition-colors hover:bg-destructive/10"
            >
              <Plus aria-hidden="true" className="h-3 w-3" />
              {tMessageList("errorActionNewSession")}
            </button>
          </>
        )}
      </div>
    ) : null

  // Goal pause/clear is a live, owner-only action, so decide availability once
  // here (where the connection is owned) rather than in the deep goal card.
  // `null` when the session isn't live or the user is a viewer → the card hides
  // its buttons. Codex is the only agent that produces goal cards, so no
  // agent-type gate is needed. Provided only around the main panel's list; the
  // read-only sub-agent dialog renders its own MessageListView with no provider.
  // The adapter's ADVERTISED goal-control vocabulary (fail-closed: no
  // controls until the snapshot for this exact connectionId reports a known
  // one — see `useAdvertisedGoalActions`). `connStatus` is what brings the
  // hook back for a second read: a brand-new conversation hands us its
  // connection id while `initialize` is still in flight, and the vocabulary
  // isn't decided until that response lands.
  const goalActions = useAdvertisedGoalActions(conn.connectionId, connStatus)

  const goalControlValue = useMemo<GoalControlValue>(() => {
    const live =
      conn.connectionId !== null &&
      (connStatus === "connected" || connStatus === "prompting") &&
      !conn.isViewer
    return {
      onGoalControl: live
        ? (action) => {
            void acpActions.goalControl(tabId, action)
          }
        : null,
      actions: goalActions,
    }
  }, [
    conn.connectionId,
    conn.isViewer,
    connStatus,
    acpActions,
    tabId,
    goalActions,
  ])

  // AIR session-failure strip actions. `retry` re-submits the LAST user
  // prompt through the message queue — same mechanism as the live-feedback
  // resend fallback: enqueue survives the turn-end status race and flushes as
  // soon as the connection can take a prompt, so the retry is never silently
  // dropped. `login` lands on the agents settings page (auth lives there);
  // `new_session` reuses the load-error banner's fresh-draft path.
  const router = useRouter()
  const tSessionFailure = useTranslations("Folder.chat.sessionFailure")
  const detailTurns = detail?.turns
  const handleSessionFailureAction = useCallback(
    (action: SessionFailureAction) => {
      switch (action) {
        case "retry": {
          // Prompt text source: the runtime TIMELINE first — it is what the
          // user sees, and a failed turn's prompt lives there as an
          // optimistic/promoted turn even when the persisted detail is stale
          // (field report 2026-08-16: after a network-drop terminal failure
          // the detail had no user turn yet, so retry read null and silently
          // did nothing). Persisted detail is the fallback for a conversation
          // whose runtime session was evicted.
          const text =
            lastUserPromptText(
              getTimelineTurns(effectiveConversationId).map((e) => e.turn)
            ) ?? lastUserPromptText(detailTurns)
          if (!text) {
            // Nothing resendable is a dead end for this action — say so
            // instead of swallowing the click.
            toast.warning(tSessionFailure("retryUnavailable"))
            return
          }
          mqEnqueue(
            { blocks: [{ type: "text", text }], displayText: text },
            selectedModeId
          )
          break
        }
        case "login":
          router.push("/settings/agents")
          break
        case "new_session":
          handleOpenNewSession()
          break
      }
    },
    [
      effectiveConversationId,
      detailTurns,
      mqEnqueue,
      selectedModeId,
      router,
      handleOpenNewSession,
      tSessionFailure,
    ]
  )

  // Closing a strip is client-local (it only resolves the record in this
  // client's projection), so unlike the recovery actions it is offered to
  // viewers too — see `AcpActionsValue.dismissSessionFailure`.
  const handleSessionFailureDismiss = useCallback(
    (ids: string[]) => {
      acpActions.dismissSessionFailures(tabId, ids)
    },
    [acpActions, tabId]
  )

  // The docked composer is the only place a quote can land, so the selection
  // bubble offers "quote" exactly when that composer is on screen (see
  // `hideInput` below). Without a composer the inject would never be consumed
  // and the action would silently do nothing.
  const composerAvailable = !isWelcomeMode && !acpLoadError

  const forkMessageRequestIdsRef = useRef(new Map<string, string>())
  useEffect(() => {
    forkMessageRequestIdsRef.current.clear()
  }, [dbConversationId])

  const handleForkFromMessage = useCallback(
    async (messageId: string) => {
      if (dbConversationId == null || !folder) return
      try {
        let requestId = forkMessageRequestIdsRef.current.get(messageId)
        if (!requestId) {
          requestId = crypto.randomUUID()
          forkMessageRequestIdsRef.current.set(messageId, requestId)
        }
        const result = await createConversationBranch({
          requestId,
          sourceConversationId: dbConversationId,
          forkMessageId: messageId,
          preferredModeId: selectedModeId,
          preferredConfigValues: Object.fromEntries(
            connectionConfigOptions.map((option) => [
              option.id,
              String(option.kind.current_value),
            ])
          ),
        })
        await refreshConversations()
        const branchTitle = useAppWorkspaceStore
          .getState()
          .conversations.find(
            (conversation) => conversation.id === result.branchConversationId
          )?.title
        openTab(
          result.folderId,
          result.branchConversationId,
          selectedAgent,
          true,
          branchTitle ?? `${ownTab?.title ?? "未命名会话"} · 分支`
        )
        forkMessageRequestIdsRef.current.delete(messageId)
        toast.success(t("branch.snapshotCreated"))
      } catch (error) {
        toast.error(toErrorMessage(error))
      }
    },
    [
      dbConversationId,
      folder,
      openTab,
      ownTab?.title,
      refreshConversations,
      selectedModeId,
      connectionConfigOptions,
      selectedAgent,
      t,
    ]
  )

  const messageListNode = (
    <GoalControlProvider value={goalControlValue}>
      <MessageListView
        conversationId={effectiveConversationId}
        agentType={selectedAgent}
        connStatus={connStatus}
        isActive={isActive}
        sendSignal={sendSignal}
        detailLoading={detailLoading}
        detailError={detailError}
        acpLoadError={acpLoadError}
        hasEarlierHistory={hasEarlierHistory}
        earlierHistoryLoading={earlierHistoryLoading}
        earlierHistoryError={earlierHistoryError}
        onLoadEarlierHistory={loadEarlierHistory}
        hideEmptyState={!hasPersistedConversation || hasSentMessage}
        onReload={canShowDetailErrorActions ? handleReloadDetail : undefined}
        onNewSession={
          canShowDetailErrorActions ? handleOpenNewSession : undefined
        }
        onQuoteSelection={composerAvailable ? handleQuoteSelection : undefined}
        deliverableRuns={detail?.deliverable_runs}
        onForkFromMessage={
          hasPersistedConversation ? handleForkFromMessage : null
        }
      />
    </GoalControlProvider>
  )

  // Live-feedback bar gating + the "agent never read your note" resend fallback.
  // Enqueue rather than `handleSend`: this fallback fires on a turn-end race
  // where the backend already reports no active turn but the frontend may still
  // read `connStatus === "prompting"`, and `handleSend` no-ops unless
  // "connected" — which would silently drop the note. The message queue holds it
  // (visible above the composer) and auto-flushes when the turn completes, so
  // the user's note is never lost.
  const feedbackEnabled = useFeedbackEnabled()
  const resendFeedbackAsPrompt = useCallback(
    (text: string) => {
      mqEnqueue(
        { blocks: [{ type: "text", text }], displayText: text },
        selectedModeId
      )
    },
    [mqEnqueue, selectedModeId]
  )
  const feedback = useSessionFeedback({
    connectionId: conn.connectionId,
    connStatus,
    enabled: feedbackEnabled,
    onResendAsPrompt: resendFeedbackAsPrompt,
  })
  // Composer "insert into current turn" (native steering only). Rethrows —
  // MessageInput owns the enqueue fallback and draft-preservation policy, so
  // this wrapper must not swallow the turn-end race the way `submit` does.
  const feedbackSteer = feedback.steer
  const handleFeedbackSteer = useCallback(
    async (text: string) => {
      await feedbackSteer(text)
    },
    [feedbackSteer]
  )

  // Deferred native guides share the existing persisted message queue but keep
  // their intent and original-turn identity. They are never allowed to fall
  // through as an ordinary next-turn prompt: after restore we either inject
  // into the same turn, or mark the guide expired for an explicit user choice.
  const guideSendInFlightRef = useRef<string | null>(null)
  useEffect(() => {
    const item = mqPeekNext("guide")
    if (!item || !item.guideTarget) return
    if (!conn.promptReady || webConnState !== "connected") return
    if (guideSendInFlightRef.current) return

    if (
      !isQueuedGuideTargetCurrent(item.guideTarget, {
        sessionId: conn.sessionId,
        connectionId: conn.connectionId,
        pendingUserMessageId: conn.pendingUserMessage?.messageId ?? null,
        status: connStatus,
      })
    ) {
      mqMarkState(item.id, "expired_guide", t("steerOriginalTurnEnded"))
      return
    }
    if (!guideConnectionReady) return

    guideSendInFlightRef.current = item.id
    void acpActions
      .steer(tabId, item.draft.blocks, item.clientMessageId)
      .then(() => {
        mqRemove(item.id)
        toast.success(t("steerInjected"))
      })
      .catch((error: unknown) => {
        const failure = classifySteerFailure(error)
        if (failure === "turn_ended") {
          mqMarkState(item.id, "expired_guide", t("steerOriginalTurnEnded"))
          return
        }
        if (
          isSessionRestorePendingError(error) ||
          isNetworkOrOfflineError(error)
        ) {
          mqMarkState(
            item.id,
            isSessionRestorePendingError(error)
              ? "waiting_session_restore"
              : "waiting_connection",
            toErrorMessage(error)
          )
          return
        }
        mqMarkState(item.id, "failed", toErrorMessage(error))
      })
      .finally(() => {
        guideSendInFlightRef.current = null
      })
  }, [
    acpActions,
    conn.connectionId,
    conn.pendingUserMessage?.messageId,
    conn.promptReady,
    conn.sessionId,
    connStatus,
    guideConnectionReady,
    mqMarkState,
    mqPeekNext,
    mqRemove,
    msgQueue,
    t,
    tabId,
    webConnState,
  ])

  const handleGuide = useCallback(
    async (draft: PromptDraft) => {
      const queueAsNextPrompt = () => {
        mqEnqueue(draft, selectedModeId)
      }
      const guideTarget = {
        sessionId: conn.sessionId,
        connectionId: conn.connectionId,
        userMessageId: conn.pendingUserMessage?.messageId ?? null,
      }
      const deferGuide = (
        clientMessageId: string,
        state: "waiting_session_restore" | "waiting_connection",
        error: string | null = null
      ) => {
        mqEnqueue(draft, selectedModeId, clientMessageId, {
          intent: "guide",
          state,
          error,
          guideTarget,
        })
      }

      // Codex app-server steering is routed as incremental text. Images and
      // embedded resources must remain intact and start the next ordinary turn;
      // never attempt a partial steer that silently drops their bytes.
      if (!draftSupportsNativeSteer(draft)) {
        queueAsNextPrompt()
        toast.info(t("imageSteerQueued"))
        return
      }

      // During a transport re-attach we still know which running turn the
      // composer represented. Persist that identity and wait for the exact
      // session snapshot instead of firing at a stale connection or silently
      // converting the guide into a future ordinary prompt.
      if (!guideConnectionReady) {
        deferGuide(`optimistic-${randomUUID()}`, "waiting_session_restore")
        toast.info(t("steerWaitingRestore"))
        return
      }

      // The status/capability may change after MessageInput rendered its Guide
      // button but before this callback runs. Preserve the whole draft as the
      // next ordinary turn rather than issuing a stale RPC.
      if (
        connStatus !== "prompting" ||
        !conn.supportsSteer ||
        !conn.connectionId
      ) {
        queueAsNextPrompt()
        toast.info(t("steerTurnEndedQueued"))
        return
      }

      const optimisticTurn = buildOptimisticUserTurnFromDraft(
        draft,
        sharedT("attachedResources")
      )
      appendOptimisticTurn(
        effectiveConversationId,
        optimisticTurn,
        optimisticTurn.id
      )
      setSendSignal((previous) => previous + 1)

      try {
        await acpActions.steer(tabId, draft.blocks, optimisticTurn.id)
        toast.success(t("steerInjected"))
      } catch (initialError: unknown) {
        let error = initialError
        let failure = classifySteerFailure(error)

        if (
          isSessionRestorePendingError(error) ||
          isNetworkOrOfflineError(error)
        ) {
          removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
          deferGuide(
            optimisticTurn.id,
            isSessionRestorePendingError(error)
              ? "waiting_session_restore"
              : "waiting_connection",
            toErrorMessage(error)
          )
          toast.info(t("steerWaitingRestore"))
          return
        }

        // A transport timeout/response loss is ambiguous: app-server may have
        // accepted the steer even though this client missed the reply. Retry
        // once with the SAME id. The backend success cache and app-server's
        // clientUserMessageId make that retry idempotent; a disconnect or a
        // deterministic protocol failure simply fails again and falls through
        // to the explicit queue recovery below.
        if (failure === "other") {
          try {
            await acpActions.steer(tabId, draft.blocks, optimisticTurn.id)
            toast.success(t("steerInjected"))
            return
          } catch (retryError: unknown) {
            error = retryError
            failure = classifySteerFailure(retryError)
          }
        }

        if (
          isSessionRestorePendingError(error) ||
          isNetworkOrOfflineError(error)
        ) {
          removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
          deferGuide(
            optimisticTurn.id,
            isSessionRestorePendingError(error)
              ? "waiting_session_restore"
              : "waiting_connection",
            toErrorMessage(error)
          )
          toast.info(t("steerWaitingRestore"))
          return
        }

        removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)

        if (failure === "unsupported") {
          // Live feedback accepts text only. Never discard images/resources to
          // force that fallback: preserve the full draft in the ordinary queue.
          const feedbackText = draft.blocks.every(
            (block) => block.type === "text"
          )
            ? draft.blocks
                .map((block) => (block.type === "text" ? block.text : ""))
                .join("\n")
                .trim()
            : ""
          if (feedback.canSubmit && feedbackText) {
            try {
              await submitSessionFeedback(conn.connectionId, feedbackText)
              toast.info(t("steerFallbackFeedback"))
              return
            } catch (feedbackError: unknown) {
              queueAsNextPrompt()
              toast.error(t("steerFailedQueued"), {
                description: toErrorMessage(feedbackError),
              })
              return
            }
          }
          queueAsNextPrompt()
          toast.info(t("steerUnsupportedQueued"))
          return
        }

        queueAsNextPrompt()
        if (failure === "turn_ended") {
          toast.info(t("steerTurnEndedQueued"))
        } else {
          toast.error(t("steerFailedQueued"), {
            description: toErrorMessage(error),
          })
        }
      }
    },
    [
      acpActions,
      appendOptimisticTurn,
      conn.connectionId,
      conn.pendingUserMessage?.messageId,
      conn.sessionId,
      conn.supportsSteer,
      connStatus,
      effectiveConversationId,
      feedback.canSubmit,
      guideConnectionReady,
      mqEnqueue,
      removeOptimisticTurn,
      selectedModeId,
      sharedT,
      t,
      tabId,
    ]
  )

  // Keep the per-tab controller above alive for background agents and events,
  // but do not mount a hidden tab's message thread and rich composer. CSS
  // visibility alone still initializes the entire React subtree (including a
  // Tiptap editor and skill/expert hooks) for every restored tab, which makes a
  // remote web client scale with the desktop client's full open-tab count.
  if (!shouldRenderContent) {
    return null
  }

  return (
    <ConversationShell
      topBanner={
        <>
          <SessionConfigStaleBanner contextKey={tabId} />
          <PiProjectTrustBanner
            contextKey={tabId}
            agentType={selectedAgent}
            workingDir={workingDirForConnection}
          />
        </>
      }
      status={isCancelling ? "cancelling" : connStatus}
      promptCapabilities={conn.promptCapabilities}
      defaultPath={workingDirForConnection}
      agentName={getAgentLabel(selectedAgent)}
      error={conn.error}
      claudeApiRetry={conn.claudeApiRetry}
      sessionFailures={conn.sessionFailures}
      onSessionFailureAction={
        // Owners of a live connection only — mirrors the goal-control gate:
        // viewers must see the strips but not drive recovery.
        conn.connectionId !== null && !conn.isViewer
          ? handleSessionFailureAction
          : undefined
      }
      onSessionFailureDismiss={handleSessionFailureDismiss}
      pendingPermission={conn.pendingPermission}
      pendingQuestion={conn.pendingQuestion}
      pendingAskQuestion={conn.pendingAskQuestion}
      pendingPlanApproval={conn.pendingPlanApproval}
      onFocus={handleFocus}
      onSend={handleSend}
      supportsSteer={conn.supportsSteer}
      onGuide={handleGuide}
      onCancel={handleCancel}
      onRespondPermission={handleRespondPermission}
      onAnswerQuestion={handleAnswerQuestion}
      onAnswerAskQuestion={handleAnswerAskQuestion}
      onAnswerPlanApproval={handleAnswerPlanApproval}
      modes={connectionModes}
      configOptions={connectionConfigOptions}
      modeLoading={modeLoading}
      configOptionsLoading={configOptionsLoading}
      selectorsLoading={selectorsLoading}
      selectedModeId={selectedModeId}
      onModeChange={handleModeChange}
      onConfigOptionChange={handleSetConfigOption}
      agentType={selectedAgent}
      availableCommands={connectionCommands}
      attachmentTabId={tabId}
      draftStorageKey={draftStorageKey}
      hideInput={isWelcomeMode || Boolean(acpLoadError)}
      injectContent={composerInject}
      onInjectConsumed={handleComposerInjectConsumed}
      composerBanner={acpLoadErrorBanner ?? null}
      feedbackList={
        feedback.showList ? (
          <FeedbackNotesDisplay notes={feedback.notes} />
        ) : null
      }
      onAddFeedback={feedback.featureEnabled ? feedback.openDialog : undefined}
      feedbackAddDisabled={!feedback.canSubmit}
      isActive={isActive}
      showActiveFlow={showActiveFlow}
      queue={msgQueue}
      onEnqueue={mqEnqueue}
      onQueueReorder={mqReorder}
      onQueueEdit={handleQueueEdit}
      onQueueDelete={mqRemove}
      onQueueRetry={handleQueueRetry}
      onConvertGuideToPrompt={mqConvertGuideToPrompt}
      editingItemId={mqEditingItemId}
      editingDraftText={editingQueueDraftText}
      editingDraftBlocks={editingQueueDraftBlocks}
      isEditingQueueItem={mqEditingItemId != null}
      onSaveQueueEdit={handleSaveQueueEdit}
      onCancelQueueEdit={handleQueueCancelEdit}
      allowOfflineCompose={hasPersistedConversation && !connectionReady}
      onForkSend={
        hasPersistedConversation && conn.supportsFork
          ? handleForkSend
          : undefined
      }
      onSteer={
        // Native channel only: on pull sessions the prompting branch must
        // stay pixel-identical (Stop button alone). The prompting scope
        // itself is enforced where the button renders.
        feedback.featureEnabled && feedback.channel === "native"
          ? handleFeedbackSteer
          : undefined
      }
    >
      {isWelcomeMode ? (
        // Same overlay scrollbar as the sidebar / file lists (os-theme-codeg)
        // instead of the platform's native bar. `min-h-full` on the inner column
        // keeps the original layout: content parked between two spacers, the
        // page scrolling only once it outgrows the viewport.
        <ScrollArea
          className="relative isolate h-full min-h-0"
          x="hidden"
          y="scroll"
        >
          <div className="flex min-h-full flex-col">
            <div className="flex-1" />
            <div className="mx-auto flex w-full max-w-3xl shrink-0 flex-col gap-6 px-4 py-4">
              <WelcomeHero />
              <QuickActions
                onSelect={handleQuickAction}
                agentType={selectedAgent}
              />
              <div className="flex justify-center">
                <AgentSelector
                  // The selector spans the row it is given (it has to measure
                  // how much room it has), so the centring lives inside it now
                  // — the `justify-center` above only centres a full-width box.
                  align="center"
                  defaultAgentType={selectedAgent}
                  onSelect={handleAgentSelect}
                  onFallback={handleAgentFallback}
                  onAgentsLoaded={(agents) => {
                    setAgentsLoaded(true)
                    setUsableAgentCount(
                      agents.filter((agent) => agent.enabled && agent.available)
                        .length
                    )
                  }}
                  onOpenAgentsSettings={handleOpenAgentsSettings}
                  disabled={isConnecting || dbConversationId != null}
                />
              </div>
              {composerBlockedMessage ? (
                <div className="flex w-full items-center gap-2 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
                  <button
                    type="button"
                    onClick={handleOpenAgentsSettings}
                    title={composerBlockedMessage}
                    className="min-w-0 flex-1 cursor-pointer overflow-hidden text-ellipsis whitespace-nowrap text-left transition-colors hover:text-destructive/80"
                  >
                    {composerBlockedMessage}
                  </button>
                  {selectedAgentNotInstalled ? (
                    <button
                      type="button"
                      onClick={() => setComposerDiagnosticsOpen(true)}
                      className="shrink-0 rounded border border-destructive/40 px-2 py-0.5 font-medium transition-colors hover:bg-destructive/10"
                    >
                      {tDiag("button")}
                    </button>
                  ) : null}
                </div>
              ) : null}
              <ChatInput
                // composerConnStatus (not connStatus): a chat draft mid-reconnect
                // reads "connecting" until the connection's cwd matches, so the
                // send affordance stays disabled until handleSend would accept it.
                status={composerConnStatus}
                promptCapabilities={conn.promptCapabilities}
                defaultPath={workingDirForConnection}
                agentName={getAgentLabel(selectedAgent)}
                onFocus={handleFocus}
                onSend={handleSend}
                onCancel={handleCancel}
                modes={connectionModes}
                configOptions={connectionConfigOptions}
                modeLoading={modeLoading}
                configOptionsLoading={configOptionsLoading}
                selectorsLoading={selectorsLoading}
                selectedModeId={selectedModeId}
                onModeChange={handleModeChange}
                onConfigOptionChange={handleSetConfigOption}
                agentType={selectedAgent}
                availableCommands={connectionCommands}
                attachmentTabId={tabId}
                draftStorageKey={draftStorageKey}
                isActive={isActive}
                showActiveFlow={showActiveFlow}
                onAddFeedback={
                  feedback.featureEnabled ? feedback.openDialog : undefined
                }
                feedbackAddDisabled={!feedback.canSubmit}
                injectContent={composerInject}
                onInjectConsumed={handleComposerInjectConsumed}
                flush
                tall
              />
            </div>
            <div className="flex-1" />
            <div className="mx-auto w-full max-w-3xl shrink-0 px-4 pb-6">
              <WelcomeTip />
            </div>
          </div>
        </ScrollArea>
      ) : showDraftHeader ? (
        <div className="flex h-full min-h-0 flex-col">
          <div className="px-4 pt-3 pb-2">
            <AgentSelector
              defaultAgentType={selectedAgent}
              onSelect={handleAgentSelect}
              onFallback={handleAgentFallback}
              onAgentsLoaded={(agents) => {
                setAgentsLoaded(true)
                setUsableAgentCount(
                  agents.filter((agent) => agent.enabled && agent.available)
                    .length
                )
              }}
              onOpenAgentsSettings={handleOpenAgentsSettings}
              disabled={isConnecting || dbConversationId != null}
            />
            {composerBlockedMessage ? (
              <div className="mt-2 flex w-full items-center gap-2 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
                <button
                  type="button"
                  onClick={handleOpenAgentsSettings}
                  title={composerBlockedMessage}
                  className="min-w-0 flex-1 cursor-pointer overflow-hidden text-ellipsis whitespace-nowrap text-left transition-colors hover:text-destructive/80"
                >
                  {composerBlockedMessage}
                </button>
                {selectedAgentNotInstalled ? (
                  <button
                    type="button"
                    onClick={() => setComposerDiagnosticsOpen(true)}
                    className="shrink-0 rounded border border-destructive/40 px-2 py-0.5 font-medium transition-colors hover:bg-destructive/10"
                  >
                    {tDiag("button")}
                  </button>
                ) : null}
              </div>
            ) : null}
          </div>
          <div className="min-h-0 flex-1">{messageListNode}</div>
        </div>
      ) : (
        messageListNode
      )}
      <FeedbackDialog
        open={feedback.dialogOpen}
        onOpenChange={(open) => {
          if (open) feedback.openDialog()
          else feedback.closeDialog()
        }}
        onSubmit={feedback.submit}
        submitting={feedback.submitting}
        agentName={getAgentLabel(selectedAgent)}
        channel={feedback.channel}
      />
      <AgentDiagnosticsDialog
        open={composerDiagnosticsOpen}
        onOpenChange={setComposerDiagnosticsOpen}
        agentType={selectedAgent}
      />
    </ConversationShell>
  )
})

// A group rect (percentages) counts as touching a container edge within this
// tolerance — ratio math can land a hair off exact 0 / 100.
const GROUP_EDGE_EPSILON = 0.1

/**
 * Corner reserve for a TOP-EDGE split-group strip. While split there is no
 * dedicated title-bar row above the shells (the workspace layout drops it
 * entirely instead of leaving a blank drag strip), so the strips along the
 * window's top edge must reserve the fixed corner overlays' width themselves —
 * exactly what the unsplit strip row does: left for LeftEdgeChrome while the
 * sidebar is collapsed (the conversation column then owns the window's left
 * edge), right for RightEdgeChrome while the column owns the right edge (aux
 * panel closed + conversation mode). Mobile shows the full-width
 * FolderTitleBar instead of corner overlays — no reserve. Self-subscribed so
 * sidebar/aux/zoom toggles re-render these slivers, not the whole panel.
 */
function SplitStripCornerReserve({ side }: { side: "left" | "right" }) {
  const isMobile = useIsMobile()
  const { isOpen: sidebarOpen } = useSidebarContext()
  const { isOpen: auxOpen } = useAuxPanelContext()
  const { mode } = useWorkspaceView()
  const { isMac, isWindows, isLinux } = usePlatform()
  const { zoomLevel } = useZoomLevel()
  if (isMobile) return null
  const width =
    side === "left"
      ? sidebarOpen
        ? 0
        : leftChromeReserve(isMac && isDesktop(), zoomLevel)
      : !auxOpen && mode === "conversation"
        ? rightChromeReserve(isDesktop() && (isWindows || isLinux), zoomLevel)
        : 0
  if (width <= 0) return null
  return (
    <div
      data-tauri-drag-region
      className="h-full shrink-0 ws-strip-line"
      style={{ width }}
    />
  )
}

export function ConversationDetailPanel() {
  const t = useTranslations("Folder.conversation")
  const tDetails = useTranslations("Folder.sessionDetails")
  const {
    completeTurn: runtimeCompleteTurn,
    loadCompleteHistory,
    removeConversation: runtimeRemoveConversation,
  } = useConversationRuntimeActions()
  const { activeFolder: folder } = useActiveFolder()
  const conversations = useAppWorkspaceStore((s) => s.conversations)
  const allFolders = useAppWorkspaceStore((s) => s.allFolders)
  const tabs = useTabStore((s) => s.tabs)
  const activeTabId = useTabStore((s) => s.activeTabId)
  const groupLayout = useTabStore((s) => s.groupLayout)
  const groupOf = useTabStore((s) => s.groupOf)
  const groupSelection = useTabStore((s) => s.groupSelection)
  const tileByGroup = useTabStore((s) => s.tileByGroup)
  // Narrow: only the drop-TARGET group id (for the shell highlight ring) — the
  // per-frame x/y writes during a drag never re-render the panel.
  const dragOverGroupId = useTabStore((s) => s.tabDrag?.overGroupId ?? null)
  const {
    openNewConversationTab,
    closeTab,
    switchTab,
    resizeGroupSplit,
    onPreviewTabReplaced,
  } = useTabActions()
  const newConversation = useMemo(() => {
    const activeTab = tabs.find((tab) => tab.id === activeTabId)
    if (!activeTab || activeTab.conversationId != null) return null
    const workingDir = activeTab.workingDir ?? folder?.path
    if (!workingDir) return null
    return { workingDir, folderId: activeTab.folderId }
  }, [tabs, activeTabId, folder?.path])
  const { disconnectIfIdle } = useAcpActions()
  const { addTask, updateTask } = useTaskContext()
  const [reloadByTabId, setReloadByTabId] = useState<Record<string, number>>({})
  const [detailsOpen, setDetailsOpen] = useState(false)

  const exportLabels = useExportLabels()

  // Release the old connection as soon as a preview tab is replaced (the next
  // single-click in the sidebar takes its slot) instead of waiting for a sweep.
  // Idle-gated on purpose: the replaced tab may hold a session that is still
  // working — often one the user only clicked in to watch — and disconnecting
  // an owner mid-turn kills the agent CLI, which lands in the transcript as an
  // interrupted request. Busy owners keep running; the idle sweep reclaims them
  // once they settle.
  useEffect(() => {
    return onPreviewTabReplaced((replacedTabId) => {
      disconnectIfIdle(replacedTabId).catch(() => {})
    })
  }, [onPreviewTabReplaced, disconnectIfIdle])

  // Background turn_complete handler: for conversations not open in tabs.
  // Subscribes via the context's primary `acp://event` listener (single
  // physical Tauri/WebSocket subscription, plus seq dedup from Phase 3b).
  // `useAcpEvent` stabilizes handler identity internally, so the callback
  // can read closure values directly — no caller-side refs needed.
  useAcpEvent(
    useCallback(
      (envelope: EventEnvelope) => {
        if (envelope.type !== "turn_complete") return

        const runtimeConversationId = getConversationIdByExternalIdFromStore(
          envelope.session_id
        )
        // Event-time read: fresher than a render capture ("`conversations`
        // may lag the tab update on fast turns" below applies to the render
        // snapshot; getState() narrows that window).
        const summary = useAppWorkspaceStore
          .getState()
          .conversations.find(
            (item) => item.external_id === envelope.session_id
          )
        const matchedConversationId =
          runtimeConversationId ?? summary?.id ?? null
        if (!matchedConversationId) return

        // Match against every identifier the panel may carry for the same
        // runtime session — otherwise this background handler races the
        // panel's own completeTurn effect and double-promotes streamingTurns
        // into localTurns (visible as a duplicated assistant message until
        // the conversation is reopened from DB).
        //
        // Invariant: `tab.runtimeConversationId` is only set when the panel's
        // effectiveConversationId differs from its bound conversationId, i.e.
        // for new conversations whose session lives under a virtual (negative)
        // id. `dbId2` is always a real DB id, so a runtimeConversationId vs.
        // dbId2 comparison is unreachable and intentionally omitted.
        // `conversations` may lag the tab update on fast turns, so dbId2
        // alone (without the runtime id branch) is not a reliable signal.
        const dbId2 = summary?.id
        const isOpenInTabs = tabs.some(
          (tab) =>
            tab.conversationId === matchedConversationId ||
            tab.runtimeConversationId === matchedConversationId ||
            (dbId2 != null && tab.conversationId === dbId2)
        )
        if (isOpenInTabs) return

        // Promote liveMessage + optimisticTurns to localTurns immediately
        runtimeCompleteTurn(matchedConversationId)

        // If tab was closed while agent was responding, clean up now.
        // Event-time read: fresh via getState(), no reactive subscription.
        const session = getRuntimeSession(matchedConversationId)
        if (session?.pendingCleanup) {
          runtimeRemoveConversation(matchedConversationId)
        }
      },
      [tabs, runtimeCompleteTurn, runtimeRemoveConversation]
    )
  )

  const hasNoTabs = tabs.length === 0 && !activeTabId
  const activeConversationTab = useMemo(
    () =>
      tabs.find(
        (tab) => tab.id === activeTabId && tab.conversationId != null
      ) ?? null,
    [tabs, activeTabId]
  )
  const canReloadActiveConversation = activeConversationTab != null
  const handleReloadActiveConversation = useCallback(() => {
    if (!activeConversationTab) return
    setReloadByTabId((prev) => ({
      ...prev,
      [activeConversationTab.id]: (prev[activeConversationTab.id] ?? 0) + 1,
    }))
  }, [activeConversationTab])

  const handleNewConversation = useCallback(() => {
    if (!folder) return
    // Right-click "new conversation" inside a conversation tab: keep the
    // active agent when the target folder has no pinned default.
    openNewConversationTab(folder.id, folder.path, { inheritFromActive: true })
  }, [folder, openNewConversationTab])

  const handleCloseActiveTab = useCallback(() => {
    if (!activeTabId) return
    closeTab(activeTabId)
  }, [activeTabId, closeTab])

  // Narrow reactive reads for the ACTIVE conversation only — a background
  // conversation's streaming token no longer re-renders this panel. `canExport`
  // keys on the tab's persisted `conversationId`; the session-details
  // resolution keys on `runtimeConversationId ?? conversationId` (a brand-new
  // conversation streams under a virtual runtime id whose live stats differ), so
  // the two are subscribed SEPARATELY — collapsing them to one lookup would
  // diverge during the virtual→persisted reconciliation window.
  const activeExportConversationId =
    activeConversationTab?.conversationId ?? null
  const canExport = useConversationRuntimeStore(
    (s) =>
      activeExportConversationId != null &&
      s.byConversationId.get(activeExportConversationId)?.detail != null
  )

  // Resolve the active conversation's summary + live token usage the same way
  // the tab view renders them — a new conversation streams under a virtual
  // `runtimeConversationId` with its usage on `sessionStats`. Extracted so the
  // resolution is unit-tested (see active-session-details.test.ts).
  const activeRuntimeId =
    activeConversationTab?.runtimeConversationId ??
    activeConversationTab?.conversationId ??
    null
  const activeRuntimeSession = useConversationRuntimeStore((s) =>
    activeRuntimeId != null
      ? (s.byConversationId.get(activeRuntimeId) ?? null)
      : null
  )
  const {
    summary: activeSessionSummary,
    stats: activeSessionStats,
    model: activeSessionModel,
  } = resolveActiveSessionDetails(
    activeConversationTab,
    // resolveActiveSessionDetails reads only `getSession(runtimeId)`, and its
    // internal `runtimeId` equals `activeRuntimeId` (identical computation), so
    // resolving that single pre-selected session is exact.
    (id) => (id === activeRuntimeId ? activeRuntimeSession : null),
    conversations
  )

  const getExportData = useCallback(async () => {
    if (activeRuntimeId == null) return null
    const detail = await loadCompleteHistory(activeRuntimeId)
    if (!detail || detail.history_page?.has_more) {
      throw new Error("Complete conversation history could not be loaded")
    }
    return {
      summary: detail.summary,
      turns: detail.turns,
      sessionStats: detail.session_stats,
      labels: exportLabels,
    }
  }, [activeRuntimeId, exportLabels, loadCompleteHistory])

  const handleExportMarkdown = useCallback(async () => {
    try {
      const data = await getExportData()
      if (!data) return
      const result = await exportAsMarkdown(data)
      if (result === "saved") toast.success(t("exportSuccess"))
      // "cancelled": user dismissed the Save dialog — stay silent,
      // matching the downloadImage / workspace-download conventions.
    } catch (err) {
      toast.error(t("exportFailed"))
      console.error("[ConversationDetailPanel] export markdown:", err)
    }
  }, [getExportData, t])

  const handleExportHtml = useCallback(async () => {
    try {
      const data = await getExportData()
      if (!data) return
      const result = await exportAsHtml(data)
      if (result === "saved") toast.success(t("exportSuccess"))
    } catch (err) {
      toast.error(t("exportFailed"))
      console.error("[ConversationDetailPanel] export html:", err)
    }
  }, [getExportData, t])

  const handleExportImage = useCallback(async () => {
    const taskId = `export-image-${Date.now()}`
    addTask(taskId, t("exportImage"))
    updateTask(taskId, { status: "running" })
    try {
      const data = await getExportData()
      if (!data) {
        updateTask(taskId, { status: "completed" })
        return
      }
      const result = await exportAsImage(data)
      updateTask(taskId, { status: "completed" })
      if (result === "saved") toast.success(t("exportSuccess"))
    } catch (err) {
      updateTask(taskId, { status: "failed" })
      if (err instanceof ExportTooLongError) {
        toast.error(t("exportImageTooLong"))
      } else {
        toast.error(t("exportFailed"))
      }
      console.error("[ConversationDetailPanel] export image:", err)
    }
  }, [getExportData, t, addTask, updateTask])

  // Ensure no-tab state is immediately bridged to a real new-conversation tab.
  useEffect(() => {
    if (!folder) return

    if (hasNoTabs) {
      openNewConversationTab(
        folder.id,
        newConversation?.workingDir ?? folder.path
      )
    }
  }, [folder, hasNoTabs, newConversation?.workingDir, openNewConversationTab])

  // Split-group render model: the layout tree only ever produces PERCENTAGE
  // RECTS — every group shell is an absolutely-positioned SIBLING keyed by its
  // stable group id, so splits/merges/orientation flips/divider drags are pure
  // style changes and a tab that stays in its group is never reparented (a
  // reparent would remount the view and tear down a live streaming response).
  const { groups: groupRects, handles: groupHandles } = useMemo(
    () => computeRects(groupLayout),
    [groupLayout]
  )
  const orderedGroupIds = useMemo(() => leafIds(groupLayout), [groupLayout])
  const isSplit = orderedGroupIds.length > 1
  const tabsByGroup = useMemo(() => {
    const byGroup = new Map<string, typeof tabs>()
    for (const groupId of orderedGroupIds) byGroup.set(groupId, [])
    for (const tab of tabs) {
      const groupId = groupOfTab(groupOf, groupLayout, tab.id)
      const bucket = byGroup.get(groupId)
      if (bucket) {
        bucket.push(tab)
      } else {
        byGroup.set(groupId, [tab])
      }
    }
    return byGroup
  }, [tabs, groupOf, groupLayout, orderedGroupIds])

  const tileTabRefs = useRef<Map<string, HTMLDivElement | null>>(new Map())
  const groupContainerRef = useRef<HTMLDivElement | null>(null)

  // Scroll each TILED group's selected tab into view when the selection (or
  // the group's tiled-ness) changes — the signature string keeps the effect
  // from refiring on unrelated tab updates.
  const tiledSelectionKey = useMemo(
    () =>
      orderedGroupIds
        .filter(
          (groupId) =>
            tileByGroup[groupId] &&
            (tabsByGroup.get(groupId)?.length ?? 0) > 1 &&
            groupSelection[groupId] != null
        )
        .map((groupId) => groupSelection[groupId])
        .join("|"),
    [orderedGroupIds, tileByGroup, tabsByGroup, groupSelection]
  )
  useEffect(() => {
    if (!tiledSelectionKey) return
    for (const selectedId of tiledSelectionKey.split("|")) {
      tileTabRefs.current.get(selectedId)?.scrollIntoView({
        behavior: "smooth",
        inline: "center",
        block: "nearest",
      })
    }
  }, [tiledSelectionKey])

  // Safety valve: if the dragged tab vanishes mid-drag (a remote snapshot
  // closing it), the drag-end callback on its unmounted item never fires —
  // clear the transient drag so the ghost/highlight can't linger.
  useEffect(() => {
    const st = useTabStore.getState()
    if (st.tabDrag && !tabs.some((tab) => tab.id === st.tabDrag?.tabId)) {
      st.endTabDrag()
    }
  }, [tabs])

  if (hasNoTabs) {
    return null
  }

  const renderTabWrapper = (
    tab: (typeof tabs)[number],
    indexInGroup: number,
    groupId: string,
    canTileG: boolean
  ) => {
    const active = tab.id === activeTabId
    // Visible = tiled (all group members shown) or the group's selected tab.
    const visible = canTileG || tab.id === groupSelection[groupId]
    const folderPath = allFolders.find((f) => f.id === tab.folderId)?.path
    const view = (
      <ConversationTabView
        tabId={tab.id}
        conversationId={tab.conversationId}
        agentType={tab.agentType}
        workingDir={tab.workingDir ?? folderPath}
        isActive={active}
        shouldLoadDetail={visible}
        shouldRenderContent={visible}
        showActiveFlow={(isSplit || canTileG) && active}
        reloadSignal={reloadByTabId[tab.id] ?? 0}
        groupId={groupId}
      />
    )
    return (
      <div
        key={tab.id}
        ref={(el) => {
          if (el) {
            tileTabRefs.current.set(tab.id, el)
          } else {
            tileTabRefs.current.delete(tab.id)
          }
        }}
        className={cn(
          canTileG
            ? cn(
                "relative h-full min-w-[24rem] flex-1 overflow-hidden",
                indexInGroup > 0 && "border-l border-border/50"
              )
            : visible
              ? "h-full"
              : "conversation-tab-hidden absolute inset-0 invisible pointer-events-none"
        )}
        onPointerDownCapture={
          visible && !active ? () => switchTab(tab.id) : undefined
        }
      >
        {/* The visible active cue is now the composer's flowing gradient border
            (see message-input.tsx); keep a non-visual cue for assistive tech
            when several sessions are visible (tiled and/or split). */}
        {(isSplit || canTileG) && active && (
          <span className="sr-only">{t("activeConversationIndicator")}</span>
        )}
        {/* A backgrounded tab is kept mounted and merely hidden (its session is
            still live), but a "查看会话" drawer opened from it portals to the
            body — so without this it went on painting over whichever tab the
            user switched to. The flag is additive, so a visible tab inside a
            covered workspace stays hidden. */}
        <OverlayHostHiddenProvider hidden={!canTileG && !visible}>
          {view}
        </OverlayHostHiddenProvider>
      </div>
    )
  }

  // Plain function (NOT a component defined in render — that type identity
  // would change every render and remount the whole shell subtree).
  const renderGroupShell = (groupId: string) => {
    const rect = groupRects.get(groupId)
    if (!rect) return null
    const groupTabs = tabsByGroup.get(groupId) ?? []
    const canTileG = !!tileByGroup[groupId] && groupTabs.length > 1
    // Only the TOP-edge strips sit under the fixed corner overlays (the split
    // layout has no title-bar row above them) — the leftmost/rightmost of that
    // row carry the corner reserves the unsplit strip row normally provides.
    const touchesTop = rect.y <= GROUP_EDGE_EPSILON
    const touchesLeft = touchesTop && rect.x <= GROUP_EDGE_EPSILON
    const touchesRight =
      touchesTop && rect.x + rect.w >= 100 - GROUP_EDGE_EPSILON
    // The group's SELECTED tab drives its header — each split group keeps the
    // full "tabs + conversation title bar" pairing of the unsplit layout.
    const selTab =
      groupTabs.find((tab) => tab.id === groupSelection[groupId]) ??
      groupTabs[0] ??
      null
    const selTabFolder = selTab
      ? allFolders.find((f) => f.id === selTab.folderId)
      : undefined
    // NOTE: the strip / header / content stay PLAIN SIBLING SLOTS (no fragment
    // around any pair) — a `false` conditional is a reconciliation hole, so the
    // content keeps its slot across split flips; wrapping would shift slots and
    // remount every live view (see group-shell-reconciliation.test.tsx).
    return (
      <div
        key={groupId}
        data-conv-group-shell={groupId}
        className="absolute flex min-h-0 flex-col overflow-hidden"
        style={{
          left: `${rect.x}%`,
          top: `${rect.y}%`,
          width: `${rect.w}%`,
          height: `${rect.h}%`,
        }}
      >
        {/* While split, each group owns its own strip (the workspace layout's
            title-bar row is gone entirely), and the TOP-edge strips add the
            corner reserves that row normally carries. */}
        {isSplit && (
          <div className="flex h-10 shrink-0 items-stretch bg-muted ws-transparent-bg">
            {touchesLeft && <SplitStripCornerReserve side="left" />}
            <TabBar groupId={groupId} />
            {touchesRight && <SplitStripCornerReserve side="right" />}
          </div>
        )}
        {isSplit && selTab && (
          <div
            className="shrink-0"
            // Clicking a non-focused group's title bar focuses that group
            // (same gesture as clicking its content) — capture phase so the
            // header's own controls still receive the event afterwards.
            onPointerDownCapture={() => {
              const selected = groupSelection[groupId]
              if (selected && selected !== useTabStore.getState().activeTabId) {
                switchTab(selected)
              }
            }}
          >
            <ConversationDetailHeader
              tabId={selTab.id}
              conversationId={selTab.conversationId}
              runtimeConversationId={selTab.runtimeConversationId ?? null}
              folderId={selTab.folderId}
              folderPath={selTabFolder?.path}
              title={selTab.title}
              status={selTab.status as ConversationStatus | undefined}
            />
          </div>
        )}
        <div className="relative min-h-0 flex-1 overflow-hidden">
          <TileScrollContainer canTile={canTileG}>
            <div
              className={cn(
                "relative h-full",
                canTileG && "flex min-w-full flex-row"
              )}
            >
              {groupTabs.map((tab, indexInGroup) =>
                renderTabWrapper(tab, indexInGroup, groupId, canTileG)
              )}
            </div>
          </TileScrollContainer>
          {/* Drop-target cue while a tab from another group hovers here. */}
          {dragOverGroupId === groupId && (
            <div className="pointer-events-none absolute inset-0 z-30 bg-primary/5 ring-2 ring-inset ring-primary/30" />
          )}
        </div>
      </div>
    )
  }

  // While UNSPLIT, a single header sits fixed above the horizontally-scrolling
  // tile row, so it never scrolls on the x-axis when conversations are tiled.
  // It reflects the ACTIVE conversation (title + owning folder). On mobile
  // there's no tile row — it's simply the sole conversation's header. While
  // SPLIT, every group shell renders its own header under its strip (the
  // "tabs + title bar" pairing per group), so the global one steps aside.
  const activeTab = tabs.find((tab) => tab.id === activeTabId) ?? null
  const activeTabFolder = activeTab
    ? allFolders.find((f) => f.id === activeTab.folderId)
    : undefined

  return (
    <>
      <div className="flex h-full min-h-0 flex-col overflow-hidden">
        {!isSplit && activeTab && (
          <ConversationDetailHeader
            tabId={activeTab.id}
            conversationId={activeTab.conversationId}
            runtimeConversationId={activeTab.runtimeConversationId ?? null}
            folderId={activeTab.folderId}
            folderPath={activeTabFolder?.path}
            title={activeTab.title}
            status={activeTab.status as ConversationStatus | undefined}
          />
        )}
        <ContextMenu>
          <ContextMenuTrigger asChild>
            <div
              ref={groupContainerRef}
              className="relative min-h-0 flex-1 overflow-hidden"
            >
              {/* Flat sibling shells keyed by stable group id + divider
                  overlays — stable across every split/tile flip, otherwise
                  sibling tabs remount and a live streaming response is torn
                  down. */}
              {orderedGroupIds.map((groupId) => renderGroupShell(groupId))}
              {isSplit &&
                groupHandles.map((handle) => (
                  <GroupSplitHandle
                    key={`${handle.splitId}:${handle.index}`}
                    handle={handle}
                    containerRef={groupContainerRef}
                    onResize={resizeGroupSplit}
                  />
                ))}
            </div>
          </ContextMenuTrigger>
          <ContextMenuContent>
            <ContextMenuItem
              disabled={!folder?.path}
              onSelect={handleNewConversation}
            >
              <SquarePen className="h-4 w-4" />
              {t("newConversation")}
            </ContextMenuItem>
            <ContextMenuSub>
              <ContextMenuSubTrigger disabled={!canExport}>
                <Download className="h-4 w-4" />
                {t("exportConversation")}
              </ContextMenuSubTrigger>
              <ContextMenuSubContent>
                <ContextMenuItem onSelect={handleExportImage}>
                  <FileImage className="h-4 w-4" />
                  {t("exportImage")}
                </ContextMenuItem>
                <ContextMenuItem onSelect={handleExportMarkdown}>
                  <FileText className="h-4 w-4" />
                  {t("exportMarkdown")}
                </ContextMenuItem>
                <ContextMenuItem onSelect={handleExportHtml}>
                  <FileCode className="h-4 w-4" />
                  {t("exportHtml")}
                </ContextMenuItem>
              </ContextMenuSubContent>
            </ContextMenuSub>
            <ContextMenuItem
              disabled={!canReloadActiveConversation}
              onSelect={handleReloadActiveConversation}
            >
              <RefreshCw className="h-4 w-4" />
              {t("reload")}
            </ContextMenuItem>
            <ContextMenuItem
              disabled={!activeSessionSummary}
              onSelect={() => setDetailsOpen(true)}
            >
              <Info className="h-4 w-4" />
              {tDetails("menuLabel")}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem
              disabled={!activeTabId}
              onSelect={handleCloseActiveTab}
            >
              <X className="h-4 w-4" />
              {t("closeConversation")}
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
      </div>
      <TabDragGhost />
      {activeSessionSummary && (
        <SessionDetailsDialog
          open={detailsOpen}
          onOpenChange={setDetailsOpen}
          summary={activeSessionSummary}
          stats={activeSessionStats}
          model={activeSessionModel}
        />
      )}
    </>
  )
}
