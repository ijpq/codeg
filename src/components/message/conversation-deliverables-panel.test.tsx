import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  copyDeliverableFiles: vi.fn(),
  downloadDeliverables: vi.fn(),
  listConversationDeliverableHistory: vi.fn(),
  openDeliverable: vi.fn(),
  revealDeliverable: vi.fn(),
  subscribe: vi.fn().mockResolvedValue(() => undefined),
}))

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))
vi.mock("@/hooks/use-deliverable-capabilities", () => ({
  useDeliverableCapabilities: () => ({
    hostOs: "windows",
    openWithDefaultApp: true,
    copyFiles: true,
    revealInFolder: true,
    hostActionNotice: true,
  }),
}))
vi.mock("@/lib/platform", () => ({ subscribe: mocks.subscribe }))
vi.mock("@/lib/api", () => ({
  copyDeliverableFiles: mocks.copyDeliverableFiles,
  downloadDeliverables: mocks.downloadDeliverables,
  listConversationDeliverableHistory: mocks.listConversationDeliverableHistory,
  openDeliverable: mocks.openDeliverable,
  revealDeliverable: mocks.revealDeliverable,
}))

import { ConversationDeliverablesPanel } from "./conversation-deliverables-panel"
import type {
  ConversationDeliverable,
  ConversationDeliverableHistoryGroup,
} from "@/lib/types"

function deliverable(
  id: string,
  fileName: string,
  overrides: Partial<ConversationDeliverable> = {}
): ConversationDeliverable {
  return {
    id,
    conversation_id: 1,
    turn_run_id: "run-1",
    root_path: "/repo",
    path: `out/${fileName}`,
    kind: "file",
    title: fileName,
    role: "primary",
    category: "standalone_output",
    change_kind: "created",
    position: 0,
    source: "declared",
    file_name: fileName,
    extension: fileName.split(".").pop() ?? null,
    size_bytes: 1024,
    is_valid: true,
    verified_at: "2026-07-18T00:00:00Z",
    produced_at: "2026-07-18T00:00:00Z",
    created_at: "2026-07-18T00:00:00Z",
    updated_at: "2026-07-18T00:00:00Z",
    ...overrides,
  }
}

function historyGroup(
  item: ConversationDeliverable,
  versions: ConversationDeliverable[] = [item]
): ConversationDeliverableHistoryGroup {
  return {
    path_key: `${item.root_path}::${item.path}`,
    latest: item,
    versions,
  }
}

function mockHistory(items: ConversationDeliverable[], total = items.length) {
  mocks.listConversationDeliverableHistory.mockResolvedValue({
    items: items.map((item) => historyGroup(item)),
    offset: 0,
    next_offset: null,
    has_more: false,
    total,
  })
}

describe("ConversationDeliverablesPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.copyDeliverableFiles.mockResolvedValue({ affected: 1 })
    mocks.downloadDeliverables.mockResolvedValue({ status: "started" })
    mocks.openDeliverable.mockResolvedValue({ affected: 1 })
    mocks.revealDeliverable.mockResolvedValue({ affected: 1 })
    mockHistory([])
  })

  it("keeps a lazy history entry without loading the ledger while collapsed", () => {
    render(
      <ConversationDeliverablesPanel
        conversationId={1}
        expanded={false}
        onToggle={vi.fn()}
      />
    )

    expect(screen.getByText("historyTitle")).toBeInTheDocument()
    expect(mocks.listConversationDeliverableHistory).not.toHaveBeenCalled()
  })

  it("loads only the persisted, server-filtered history page when expanded", async () => {
    mockHistory([
      deliverable("docx", "报告.docx"),
      deliverable("pdf", "报告.pdf", { source: "inferred" }),
    ])
    render(
      <ConversationDeliverablesPanel
        conversationId={1}
        expanded
        onToggle={vi.fn()}
      />
    )

    expect(await screen.findByText("报告.docx")).toBeInTheDocument()
    expect(screen.getByText("报告.pdf")).toBeInTheDocument()
    expect(screen.getByText("inferred")).toBeInTheDocument()
    expect(mocks.listConversationDeliverableHistory).toHaveBeenCalledWith(
      1,
      0,
      25
    )
  })

  it("shows a deduplicated path and expands its per-turn version lineage", async () => {
    const latest = deliverable("same", "报告.pdf", {
      turn_run_id: "run-2",
      produced_at: "2026-07-19T00:00:00Z",
    })
    const older = deliverable("same", "报告.pdf", {
      turn_run_id: "run-1",
      produced_at: "2026-07-18T00:00:00Z",
    })
    mocks.listConversationDeliverableHistory.mockResolvedValue({
      items: [historyGroup(latest, [latest, older])],
      offset: 0,
      next_offset: null,
      has_more: false,
      total: 1,
    })
    render(
      <ConversationDeliverablesPanel
        conversationId={1}
        expanded
        onToggle={vi.fn()}
      />
    )

    await screen.findByText("报告.pdf")
    fireEvent.click(screen.getByText("versions"))
    expect(screen.getAllByText(/declared/)).toHaveLength(2)
  })

  it("keeps long history bounded and loads the next page on demand", async () => {
    const first = deliverable("first", "第一页.pdf")
    const second = deliverable("second", "下一页.pdf")
    mocks.listConversationDeliverableHistory
      .mockResolvedValueOnce({
        items: [historyGroup(first)],
        offset: 0,
        next_offset: 25,
        has_more: true,
        total: 158,
      })
      .mockResolvedValueOnce({
        items: [historyGroup(second)],
        offset: 25,
        next_offset: null,
        has_more: false,
        total: 158,
      })
    render(
      <ConversationDeliverablesPanel
        conversationId={28}
        expanded
        onToggle={vi.fn()}
      />
    )

    await screen.findByText("第一页.pdf")
    expect(screen.queryByText("下一页.pdf")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "loadMore" }))
    expect(await screen.findByText("下一页.pdf")).toBeInTheDocument()
    expect(mocks.listConversationDeliverableHistory).toHaveBeenLastCalledWith(
      28,
      25,
      25
    )
  })

  it("downloads by deliverable id without sending a source path", async () => {
    mockHistory([deliverable("docx-id", "交付 文档.docx")])
    render(
      <ConversationDeliverablesPanel
        conversationId={7}
        expanded
        onToggle={vi.fn()}
      />
    )

    await screen.findByText("交付 文档.docx")
    fireEvent.click(screen.getByRole("button", { name: "download" }))
    await waitFor(() => {
      expect(mocks.downloadDeliverables).toHaveBeenCalledWith({
        conversationId: 7,
        deliverableIds: ["docx-id"],
        archive: false,
        suggestedName: "交付 文档.docx",
      })
    })
  })

  it("opens with the host default application by deliverable id", async () => {
    mockHistory([deliverable("pdf-id", "最终 报告.pdf")])
    render(
      <ConversationDeliverablesPanel
        conversationId={7}
        expanded
        onToggle={vi.fn()}
      />
    )

    await screen.findByText("最终 报告.pdf")
    fireEvent.click(
      screen.getByRole("button", { name: "openWithDefaultAppHost" })
    )
    await waitFor(() => {
      expect(mocks.openDeliverable).toHaveBeenCalledWith(7, "pdf-id")
    })
  })

  it("copies selected files as one host clipboard operation", async () => {
    mockHistory([deliverable("a", "A.pdf"), deliverable("b", "B.pdf")])
    render(
      <ConversationDeliverablesPanel
        conversationId={9}
        expanded
        onToggle={vi.fn()}
      />
    )

    await screen.findByText("A.pdf")
    for (const checkbox of screen.getAllByRole("checkbox", {
      name: "selectFile",
    })) {
      fireEvent.click(checkbox)
    }
    fireEvent.click(screen.getByRole("button", { name: "copySelectedHost" }))

    await waitFor(() => {
      expect(mocks.copyDeliverableFiles).toHaveBeenCalledWith(9, ["a", "b"])
    })
  })
})
