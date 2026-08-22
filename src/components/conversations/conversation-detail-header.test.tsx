import { type ComponentProps, type ReactElement } from "react"
import { render, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi, beforeEach } from "vitest"

import enMessages from "@/i18n/messages/en.json"

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
  createConversationBranch: vi.fn(async () => ({
    branchConversationId: 9,
    sourceConversationId: 1,
    folderId: 1,
    connectionId: "branch-connection",
    branchSessionId: "branch-session",
    sessionReady: true,
    promptReady: true,
    lifecycleState: "ready",
    forkMode: "native" as const,
    inheritanceMode: "native_fork" as const,
    inheritedMessageCount: 12,
    inheritanceTruncated: false,
  })),
  getConversationBranchInfo: vi.fn(async () => null),
  listConversationDeliverables: vi.fn(async () => []),
  mergeConversationBranch: vi.fn(async () => ({
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
}))

vi.mock("@/lib/api", () => ({
  updateConversationTitle: h.updateConversationTitle,
  deleteConversation: h.deleteConversation,
  updateConversationStatus: h.updateConversationStatus,
  updateConversationPinned: h.updateConversationPinned,
  createConversationBranch: h.createConversationBranch,
  getConversationBranchInfo: h.getConversationBranchInfo,
  listConversationDeliverables: h.listConversationDeliverables,
  mergeConversationBranch: h.mergeConversationBranch,
}))
vi.mock("@/contexts/tab-context", () => ({
  useTabActions: () => ({
    closeTab: h.closeTab,
    openTab: h.openTab,
    openNewConversationTab: h.openNewConversationTab,
  }),
}))
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
      expect(h.closeTab).toHaveBeenCalledWith("tab-a")
    })
    expect(h.deleteConversation).not.toHaveBeenCalledWith(2)
    expect(h.closeTab).not.toHaveBeenCalledWith("tab-b")
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
        "conv-a · 分支"
      )
    })
  })

  it("accepts the fork-send two-row mapping when the live source becomes the branch", async () => {
    h.createConversationBranch.mockResolvedValueOnce({
      branchConversationId: 1,
      sourceConversationId: 9,
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
        [1, 9, "codex", true, "conv-a"],
        [1, 1, "codex", true, "conv-a · 分支"],
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
    expect(h.openTab).toHaveBeenCalledWith(1, 9, "codex", true, "conv-a · 分支")
  })

  it("refreshes branch controls when fork-send repoints the active conversation", async () => {
    h.getConversationBranchInfo
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce({
        branchConversationId: 1,
        sourceConversationId: 9,
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
        lastConnectionId: "connection-fork",
        snapshotDigest: null,
        createdAt: "2026-08-22T00:00:00Z",
        lastMergedAt: null,
        mergeTargetConversationId: null,
      })
    const { rerender, findByRole } = render(
      withIntl(<ConversationDetailHeader {...A} />)
    )
    await waitFor(() => {
      expect(h.getConversationBranchInfo).toHaveBeenCalledTimes(1)
    })

    // Fork & Send changes the row's title/external session in the workspace
    // refresh without changing its DB id. That transition must still re-read
    // the durable branch relation so Return/Merge appear without a reload.
    rerender(
      withIntl(<ConversationDetailHeader {...A} title="[Fork] conv-a" />)
    )

    expect(
      await findByRole("button", { name: /Branched from: conv-a/ })
    ).toBeInTheDocument()
    await userEvent
      .setup({ pointerEventsCheck: 0 })
      .click(await findByRole("button", { name: "More actions" }))
    expect(
      await findByRole("menuitem", { name: "Merge into main conversation" })
    ).toBeInTheDocument()
  })
})
