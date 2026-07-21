import { render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))
vi.mock("./deliverable-file-actions", () => ({
  DeliverableFileActions: ({ item }: { item: { id: string } }) => (
    <button>{`actions-${item.id}`}</button>
  ),
}))

import { ReplyDeliverables } from "./reply-deliverables"
import type { ConversationDeliverable } from "@/lib/types"

function item(
  id: string,
  name: string,
  overrides: Partial<ConversationDeliverable> = {}
): ConversationDeliverable {
  return {
    id,
    conversation_id: 11,
    turn_run_id: "run-11",
    root_path: "/repo",
    path: name,
    kind: "file",
    title: name,
    role: "primary",
    position: 0,
    source: "declared",
    file_name: name,
    extension: name.split(".").pop(),
    size_bytes: 123,
    is_valid: true,
    verified_at: "2026-07-20T00:00:00Z",
    produced_at: "2026-07-20T00:00:00Z",
    created_at: "2026-07-20T00:00:00Z",
    updated_at: "2026-07-20T00:00:00Z",
    ...overrides,
  }
}

describe("ReplyDeliverables", () => {
  it("renders no empty artifact region", () => {
    const { container } = render(
      <ReplyDeliverables conversationId={11} deliverables={[]} />
    )
    expect(container).toBeEmptyDOMElement()
  })

  it("shows only the confirmed set for this turn with role and inference labels", () => {
    render(
      <ReplyDeliverables
        conversationId={11}
        deliverables={[
          item("docx", "结果.docx"),
          item("pdf", "结果.pdf", {
            role: "supporting",
            source: "inferred",
            is_valid: false,
            invalid_reason: "file_not_found",
          }),
        ]}
      />
    )
    expect(screen.getByText("结果.docx")).toBeInTheDocument()
    expect(screen.getByText("结果.pdf")).toBeInTheDocument()
    expect(screen.getByText("primary")).toBeInTheDocument()
    expect(screen.getByText("supporting")).toBeInTheDocument()
    expect(screen.getByText("inferred")).toBeInTheDocument()
    expect(screen.getByText("missing")).toBeInTheDocument()
    expect(screen.queryByText("input.pdf")).not.toBeInTheDocument()
  })
})
