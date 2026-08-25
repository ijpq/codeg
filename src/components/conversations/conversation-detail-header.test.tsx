import { type ComponentProps, type ReactElement } from "react"
import { act, render, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi, beforeEach } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import type {
  ConversationBranchInfo,
  CreateConversationBranchRequest,
  CreateConversationBranchResult,
  MergeConversationBranchResult,
} from "@/lib/api"

// The header is a SINGLE instance reused across active tabs, and the global
// tab-switch / close-tab shortcuts still fire while a rename/delete dialog is
// open. These tests pin the regression Codex flagged: a confirm must act on the
// conversation the dialog was OPENED for, not whatever is active at confirm
// time. We open the dialog for A, rerender the same instance as B (simulating a
// mid-dialog tab switch), then confirm — and assert A is mutated, never B.
const h = vi.hoisted(() => ({
  updateConversationTitle: vi.fn(async () => {}),
  deleteConversation: vi.fn(async () => {}),
  updateConversationStatus: vi.fn(async () => {}),
  updateConversationPinned: vi.fn(async () => {}),
  createConversationBranch: vi.fn<
    (
      request: CreateConversationBranchRequest
    ) => Promise<CreateConversationBranchResult>
  >(async () => ({
    branchConversationId: 9,
    sourceConversationId: 1,
    folderId: 1,
    connectionId: "branch-connection",
    branchSessionId: "branch-session",
    sessionReady: true,
    promptReady: true,
    lifecycleState: "ready",
    forkMode: "native",
    inheritanceMode: "native_fork",
    inheritedMessageCount: 12,
    inheritanceTruncated: false,
  })),
  getConversationBranchInfo: vi.fn<
    (conversationId: number) => Promise<ConversationBranchInfo | null>
  >(async () => null),
  mergeConversationBranch: vi.fn<
    (request: {
      branchConversationId: number
      requestId: string
    }) => Promise<MergeConversationBranchResult>
  >(async () => ({
    mergeId: "merge-1",
    targetConversationId: 1,
    copiedDeliverableCount: 0,
    deduplicated: false,
  })),
  closeTab: vi.fn(),
  openTab: vi.fn(),
  openNewConversationTab: vi.fn(),
  updateConversationLocal: vi.fn(),
  refreshConversations: vi.fn(),
  mergeProgressHandler: null as null | ((payload: unknown) => void),
  subscribe: vi.fn(
    async (_event: string, handler: (payload: unknown) => void) => {
      h.mergeProgressHandler = handler
      return () => {}
    }
  ),
}))

vi.mock("@/lib/api", () => ({
  updateConversationTitle: h.updateConversationTitle,
  deleteConversation: h.deleteConversation,
  updateConversationStatus: h.updateConversationStatus,
  updateConversationPinned: h.updateConversationPinned,
  createConversationBranch: h.createConversationBranch,
  getConversationBranchInfo: h.getConversationBranchInfo,
  mergeConversationBranch: h.mergeConversationBranch,
}))
vi.mock("@/contexts/tab-context", () => ({
  useTabActions: () => ({
    closeTab: h.closeTab,
    openTab: h.openTab,
    openNewConversationTab: h.openNewConversationTab,
  }),
}))
vi.mock("@/lib/platform", () => ({ subscribe: h.subscribe }))
vi.mock("@/stores/app-workspace-store", () => {
  const state = {
    updateConversationLocal: h.updateConversationLocal,
    refreshConversations: h.refreshConversations,
    conversations: [
      { id: 1, folder_id: 1, agent_type: "codex", title: "conv-a" },
      { id: 2, folder_id: 1, agent_type: "codex", title: "conv-b" },
    ],
  }
  const useStore = (selector: (s: typeof state) => unknown) => selector(state)
  useStore.getState = () => state
  return { useAppWorkspaceStore: useStore }
})
vi.mock("@/stores/conversation-runtime-store", () => ({
  getRuntimeSession: () => ({ detail: { turns: [] }, localTurns: [] }),
}))
vi.mock("@/contexts/acp-connections-context", () => ({
  getCachedSelectors: () => null,
}))
vi.mock("./session-details-dialog", () => ({
  SessionDetailsDialog: () => null,
}))
// The header now embeds the folder picker (self-contained, store-driven); stub
// it so these tests exercise only the header's own menu/dialog logic.
vi.mock("@/components/chat/conversation-context-bar", () => ({
  ConversationHeaderFolderPicker: () => null,
}))

import { ConversationDetailHeader } from "./conversation-detail-header"

type Props = ComponentProps<typeof ConversationDetailHeader>

const A: Props = {
  tabId: "tab-a",
  conversationId: 1,
  runtimeConversationId: null,
  folderId: 1,
  folderPath: "/a",
  title: "conv-a",
  status: "in_progress",
}
const B: Props = {
  ...A,
  tabId: "tab-b",
  conversationId: 2,
  title: "conv-b",
}

function withIntl(ui: ReactElement) {
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

describe("ConversationDetailHeader dialog target snapshot", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    h.mergeProgressHandler = null
  })

  it("deletes the conversation the dialog was opened for, even after the active tab switches", async () => {
    // pointerEventsCheck off: Radix toggles body pointer-events while a menu is
    // open, which user-event's default guard would trip on in jsdom.
    const user = userEvent.setup({ pointerEventsCheck: 0 })
    const { rerender, getByLabelText, getByRole } = render(
      withIntl(<ConversationDetailHeader {...A} />)
    )

    await user.click(getByLabelText("More actions"))
    await user.click(getByRole("menuitem", { name: "Delete" }))

    // Simulate a mid-dialog tab switch: same header instance, now scoped to B.
    rerender(withIntl(<ConversationDetailHeader {...B} />))

    await user.click(getByRole("button", { name: "Delete" }))

    await waitFor(() => {
      expect(h.deleteConversation).toHaveBeenCalledWith(1)
      // `recordForReopen: false`: the row is deleted, so "reopen closed tab"
      // must not be able to mint a tab pointing back at it.
      expect(h.closeTab).toHaveBeenCalledWith("tab-a", {
        recordForReopen: false,
      })
    })
    expect(h.deleteConversation).not.toHaveBeenCalledWith(2)
    expect(h.closeTab).not.toHaveBeenCalledWith("tab-b", expect.anything())
  })

  it("renames the conversation the dialog was opened for, even after the active tab switches", async () => {
    const user = userEvent.setup({ pointerEventsCheck: 0 })
    const { rerender, getByLabelText, getByRole } = render(
      withIntl(<ConversationDetailHeader {...A} />)
    )

    await user.click(getByLabelText("More actions"))
    await user.click(getByRole("menuitem", { name: "Rename" }))

    rerender(withIntl(<ConversationDetailHeader {...B} />))

    const input = getByRole("textbox")
    await user.clear(input)
    await user.type(input, "renamed")
    await user.click(getByRole("button", { name: "Save" }))

    await waitFor(() => {
      expect(h.updateConversationTitle).toHaveBeenCalledWith(1, "renamed")
    })
    expect(h.updateConversationTitle).not.toHaveBeenCalledWith(2, "renamed")
  })

  it("creates and opens an independent branch from the conversation menu", async () => {
    const user = userEvent.setup({ pointerEventsCheck: 0 })
    const { getByLabelText, getByRole } = render(
      withIntl(<ConversationDetailHeader {...A} />)
    )

    await user.click(getByLabelText("More actions"))
    await user.click(getByRole("menuitem", { name: "Create branch" }))

    await waitFor(() => {
      expect(h.createConversationBranch).toHaveBeenCalledWith(
        expect.objectContaining({ sourceConversationId: 1 })
      )
      expect(h.createConversationBranch.mock.calls[0]?.[0]).not.toHaveProperty(
        "snapshotContext"
      )
      expect(h.openTab).toHaveBeenCalledWith(
        1,
        9,
        "codex",
        true,
        expect.stringMatching(/^conv-a · 分支 \d{1,2}\.\d{1,2}$/)
      )
    })
  })

  it("hands Create Branch to the durable tab queue when the runtime accepts it", async () => {
    const queued: CustomEvent[] = []
    const listener = (event: Event) => {
      queued.push(event as CustomEvent)
      event.preventDefault()
    }
    window.addEventListener(
      "codeg:queue-conversation-branch-creation",
      listener
    )
    const user = userEvent.setup({ pointerEventsCheck: 0 })
    const { getByLabelText, getByRole } = render(
      withIntl(<ConversationDetailHeader {...A} />)
    )

    await user.click(getByLabelText("More actions"))
    await user.click(getByRole("menuitem", { name: "Create branch" }))

    expect(queued).toHaveLength(1)
    expect(queued[0]!.detail).toMatchObject({
      conversationId: 1,
      requestId: expect.any(String),
      operationId: expect.any(String),
    })
    expect(queued[0]!.detail.operationId).toBe(queued[0]!.detail.requestId)
    expect(h.createConversationBranch).not.toHaveBeenCalled()

    await user.click(getByLabelText("More actions"))
    await user.click(getByRole("menuitem", { name: "Create branch" }))
    expect(queued).toHaveLength(2)
    expect(queued[1]!.detail.operationId).not.toBe(
      queued[0]!.detail.operationId
    )
    window.removeEventListener(
      "codeg:queue-conversation-branch-creation",
      listener
    )
  })

  it("keeps the source row unchanged and opens only the new branch", async () => {
    h.createConversationBranch.mockResolvedValueOnce({
      branchConversationId: 9,
      sourceConversationId: 1,
      folderId: 1,
      connectionId: "branch-connection",
      branchSessionId: "branch-session",
      sessionReady: true,
      promptReady: true,
      lifecycleState: "ready",
      forkMode: "native",
      inheritanceMode: "native_fork",
      inheritedMessageCount: 12,
      inheritanceTruncated: false,
    })
    const user = userEvent.setup({ pointerEventsCheck: 0 })
    const { getByLabelText, getByRole } = render(
      withIntl(<ConversationDetailHeader {...A} />)
    )

    await user.click(getByLabelText("More actions"))
    await user.click(getByRole("menuitem", { name: "Create branch" }))

    await waitFor(() => {
      expect(h.openTab.mock.calls).toEqual([
        [
          1,
          9,
          "codex",
          true,
          expect.stringMatching(/^conv-a · 分支 \d{1,2}\.\d{1,2}$/),
        ],
      ])
    })
  })

  it("opens a provisional snapshot branch before its first ACP prompt is ready", async () => {
    h.createConversationBranch.mockResolvedValueOnce({
      branchConversationId: 9,
      sourceConversationId: 1,
      folderId: 1,
      connectionId: null,
      branchSessionId: null,
      sessionReady: false,
      promptReady: false,
      lifecycleState: "provisional",
      forkMode: "snapshot",
      inheritanceMode: "full_replay",
      inheritedMessageCount: 12,
      inheritanceTruncated: false,
    })
    const user = userEvent.setup({ pointerEventsCheck: 0 })
    const { getByLabelText, getByRole } = render(
      withIntl(<ConversationDetailHeader {...A} />)
    )

    await user.click(getByLabelText("More actions"))
    await user.click(getByRole("menuitem", { name: "Create branch" }))

    await waitFor(() => {
      expect(h.createConversationBranch).toHaveBeenCalledTimes(1)
    })
    expect(h.refreshConversations).toHaveBeenCalled()
    expect(h.openTab).toHaveBeenCalledWith(
      1,
      9,
      "codex",
      true,
      expect.stringMatching(/^conv-a · 分支 \d{1,2}\.\d{1,2}$/)
    )
  })

  it("returns to the source in one click without a content-selection dialog", async () => {
    let finishMerge!: (value: MergeConversationBranchResult) => void
    h.mergeConversationBranch.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishMerge = resolve
        })
    )
    h.getConversationBranchInfo.mockResolvedValueOnce({
      branchConversationId: 2,
      sourceConversationId: 1,
      sourceTitle: "conv-a",
      sourceAvailable: true,
      forkMessageId: null,
      forkMode: "native",
      sourceSessionId: "session-source",
      branchSessionId: "session-fork",
      inheritanceMode: "native_fork",
      inheritedMessageCount: 12,
      inheritedContextChars: 0,
      inheritedEstimatedTokens: 0,
      inheritanceCompressed: false,
      inheritanceTruncated: false,
      inheritanceNote: null,
      forkedThroughAt: "2026-08-22T00:00:00Z",
      snapshotVersion: 2,
      snapshotConsumedAt: null,
      lifecycleState: "ready",
      lifecycleError: null,
      lifecycleUpdatedAt: "2026-08-22T00:00:00Z",
      sessionVerifiedAt: "2026-08-22T00:00:00Z",
      firstPromptClientMessageId: null,
      firstPromptQueuedAt: null,
      firstPromptAcceptedAt: null,
      initializationRetryCount: 0,
      lastConnectionId: "connection-branch",
      snapshotDigest: null,
      createdAt: "2026-08-22T00:00:00Z",
      lastMergedAt: null,
      mergeTargetConversationId: null,
    })
    const user = userEvent.setup({ pointerEventsCheck: 0 })
    const { findByRole, queryByRole } = render(
      withIntl(<ConversationDetailHeader {...B} />)
    )

    await user.click(await findByRole("button", { name: "More actions" }))
    await user.click(
      await findByRole("menuitem", { name: "Merge into main conversation" })
    )

    await waitFor(() => {
      expect(h.mergeConversationBranch).toHaveBeenCalledWith({
        branchConversationId: 2,
        requestId: expect.any(String),
      })
    })
    const requestId = h.mergeConversationBranch.mock.calls[0]![0].requestId
    act(() => {
      h.mergeProgressHandler?.({
        branchConversationId: 2,
        requestId,
        stage: "extracting_increment",
      })
    })
    expect(await findByRole("status")).toHaveTextContent(
      "Extracting branch changes…"
    )

    finishMerge({
      mergeId: "merge-progress",
      targetConversationId: 1,
      copiedDeliverableCount: 0,
      deduplicated: false,
    })
    await waitFor(() => {
      expect(h.openTab).toHaveBeenCalledWith(1, 1, "codex", true, "conv-a")
      expect(h.closeTab).toHaveBeenCalledWith("tab-b")
    })
    expect(queryByRole("dialog")).not.toBeInTheDocument()
  })
})
