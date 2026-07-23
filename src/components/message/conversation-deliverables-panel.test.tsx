import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  copyDeliverableFiles: vi.fn(),
  downloadDeliverables: vi.fn(),
  hideDeliverables: vi.fn(),
  openDeliverable: vi.fn(),
  revealDeliverable: vi.fn(),
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
vi.mock("@/lib/api", () => ({
  copyDeliverableFiles: mocks.copyDeliverableFiles,
  downloadDeliverables: mocks.downloadDeliverables,
  hideDeliverables: mocks.hideDeliverables,
  openDeliverable: mocks.openDeliverable,
  revealDeliverable: mocks.revealDeliverable,
}))

import { ConversationDeliverablesPanel } from "./conversation-deliverables-panel"
import type { ConversationDeliverable } from "@/lib/types"

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

describe("ConversationDeliverablesPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.copyDeliverableFiles.mockResolvedValue({ affected: 1 })
    mocks.downloadDeliverables.mockResolvedValue({ status: "started" })
    mocks.hideDeliverables.mockResolvedValue({ affected: 1 })
    mocks.openDeliverable.mockResolvedValue({ affected: 1 })
    mocks.revealDeliverable.mockResolvedValue({ affected: 1 })
  })

  it("keeps a fixed conversation entry even when the list is empty", () => {
    render(
      <ConversationDeliverablesPanel
        conversationId={1}
        expanded={false}
        onToggle={vi.fn()}
        deliverables={[]}
      />
    )

    expect(screen.getByText("collapsedSummary")).toBeInTheDocument()
  })

  it("renders only persisted deliverables and marks inferred and missing files", () => {
    render(
      <ConversationDeliverablesPanel
        conversationId={1}
        expanded
        onToggle={vi.fn()}
        deliverables={[
          deliverable("docx", "报告.docx"),
          deliverable("pdf", "报告.pdf", {
            source: "inferred",
            is_valid: false,
            invalid_reason: "file_not_found",
          }),
        ]}
      />
    )

    expect(screen.getByText("报告.docx")).toBeInTheDocument()
    expect(screen.getByText("报告.pdf")).toBeInTheDocument()
    expect(screen.getByText("inferred")).toBeInTheDocument()
    expect(screen.getByText("missing")).toBeInTheDocument()
    expect(screen.queryByText("package.json")).not.toBeInTheDocument()
  })

  it("downloads by deliverable id without sending a source path", async () => {
    render(
      <ConversationDeliverablesPanel
        conversationId={7}
        expanded
        onToggle={vi.fn()}
        deliverables={[deliverable("docx-id", "交付 文档.docx")]}
      />
    )

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
    render(
      <ConversationDeliverablesPanel
        conversationId={7}
        expanded
        onToggle={vi.fn()}
        deliverables={[deliverable("pdf-id", "最终 报告.pdf")]}
      />
    )

    fireEvent.click(
      screen.getByRole("button", { name: "openWithDefaultAppHost" })
    )
    await waitFor(() => {
      expect(mocks.openDeliverable).toHaveBeenCalledWith(7, "pdf-id")
    })
  })

  it("copies selected files as one host clipboard operation", async () => {
    render(
      <ConversationDeliverablesPanel
        conversationId={9}
        expanded
        onToggle={vi.fn()}
        deliverables={[deliverable("a", "A.pdf"), deliverable("b", "B.pdf")]}
      />
    )

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
