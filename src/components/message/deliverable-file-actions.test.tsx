import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import type { ConversationDeliverable } from "@/lib/types"

vi.mock("next-intl", () => ({ useTranslations: () => (key: string) => key }))
vi.mock("@/hooks/use-deliverable-capabilities", () => ({
  useDeliverableCapabilities: () => ({}),
}))
vi.mock("@/lib/api", () => ({
  downloadDeliverables: vi.fn().mockResolvedValue(undefined),
  copyDeliverableFiles: vi.fn(),
  openDeliverable: vi.fn(),
  revealDeliverable: vi.fn(),
  getOfficeDeliveryPreview: vi.fn(),
}))
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }))
const workspace = vi.hoisted(() => ({
  openFilePreview: vi.fn(),
  openOfficeDeliveryPreview: vi.fn(),
}))
vi.mock("@/contexts/workspace-context", () => ({
  useOptionalWorkspaceActions: () => workspace,
}))
import { getOfficeDeliveryPreview, downloadDeliverables } from "@/lib/api"
import { DeliverableFileActions } from "./deliverable-file-actions"

describe("inherited deliverable file access", () => {
  it("downloads through the verified owning conversation instead of the branch", async () => {
    const item = {
      id: "source-file",
      conversation_id: 12,
      turn_run_id: "source-run",
      kind: "file",
      file_name: "report.pdf",
      path: "report.pdf",
      root_path: "/workspace",
      is_valid: true,
    } as ConversationDeliverable
    render(<DeliverableFileActions conversationId={31} item={item} />)
    fireEvent.click(screen.getByRole("button", { name: "download" }))
    await waitFor(() =>
      expect(downloadDeliverables).toHaveBeenCalledWith({
        conversationId: 12,
        deliverableIds: ["source-file"],
        archive: false,
        suggestedName: "report.pdf",
      })
    )
  })
})

describe("manual Office review", () => {
  beforeEach(() => vi.clearAllMocks())
  const item = {
    id: "office",
    conversation_id: 12,
    turn_run_id: "run-1",
    kind: "file",
    file_name: "report.docx",
    path: "report.docx",
    root_path: "/work",
    is_valid: true,
  } as ConversationDeliverable
  it("opens the correct captured version in CodeG", async () => {
    const review = {
      status: "ready",
      deliverable_id: "office",
      snapshot_path: "/private/report.docx",
    }
    vi.mocked(getOfficeDeliveryPreview).mockResolvedValue(review as never)
    render(<DeliverableFileActions conversationId={31} item={item} />)
    fireEvent.click(screen.getByRole("button", { name: "previewInCodeg" }))
    await waitFor(() =>
      expect(workspace.openOfficeDeliveryPreview).toHaveBeenCalledWith(review)
    )
    expect(getOfficeDeliveryPreview).toHaveBeenCalledWith(12, "run-1")
    expect(workspace.openFilePreview).not.toHaveBeenCalled()
  })
  it("an uncaptured historical file only offers an explicitly current manual view", async () => {
    vi.mocked(getOfficeDeliveryPreview).mockResolvedValue(null)
    render(<DeliverableFileActions conversationId={31} item={item} />)
    fireEvent.click(screen.getByRole("button", { name: "previewInCodeg" }))
    await waitFor(() =>
      expect(workspace.openFilePreview).toHaveBeenCalledWith(
        "/work/report.docx",
        { reload: true }
      )
    )
    expect(workspace.openOfficeDeliveryPreview).not.toHaveBeenCalled()
  })
})
