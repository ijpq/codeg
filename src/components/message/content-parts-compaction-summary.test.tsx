import { type ReactNode } from "react"
import { render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"

/**
 * The compaction card inside a live turn reads its retained summary off the
 * tool call's output — but ONLY when the backend claimed that output as the
 * summary. claude's legacy compaction call puts its metadata object on the
 * very same channel, and that must never be offered as a "Summary".
 */

vi.mock("@/components/ai-elements/link-safety", () => ({
  FilePathLink: ({ children }: { children: ReactNode }) => (
    <span>{children}</span>
  ),
  useStreamdownLinkSafety: () => ({ enabled: false }),
}))

vi.mock("@/components/ai-elements/code-block", () => ({
  CodeBlock: ({ code }: { code: string }) => <pre>{code}</pre>,
}))

vi.mock("@/components/ai-elements/message", () => ({
  MessageResponse: ({ children }: { children: string }) => (
    <div>{children}</div>
  ),
}))

import { ContentPartsRenderer } from "./content-parts-renderer"
import enMessages from "@/i18n/messages/en.json"
import { adaptMessageTurn } from "@/lib/adapters/ai-elements-adapter"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import { COMPACTION_SUMMARY_META_KEY } from "@/lib/context-compaction"

function compactionPart(
  meta: Record<string, unknown>,
  output: string
): AdaptedContentPart {
  return {
    type: "tool-call",
    toolCallId: "cmp_1",
    toolName: "Context compaction",
    input: null,
    state: "output-available",
    output,
    meta,
  }
}

function renderParts(parts: AdaptedContentPart[]) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <ContentPartsRenderer parts={parts} role="assistant" />
    </NextIntlClientProvider>
  )
}

describe("compaction summary in a live turn", () => {
  it("keeps restored native compaction active without a live tool subscription", () => {
    const adapted = adaptMessageTurn(
      {
        id: "codeg-compaction:pi-session:operation",
        role: "assistant",
        timestamp: "2026-10-30T22:04:53.332Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "operation",
            tool_name: "context_compaction",
            input_preview: null,
            status: "in_progress",
            meta: {
              contextCompaction: {
                version: 1,
                status: "in_progress",
                phase: "summarizing",
                eventId: "operation",
              },
            },
          },
        ],
      },
      {
        attachedResources: "Attached resources",
        toolCallFailed: "Tool failed",
        pageHandoffName: () => "Continue here",
      }
    )
    renderParts(adapted.content)
    expect(screen.getByTestId("context-compaction")).toHaveAttribute(
      "data-compaction-status",
      "in_progress"
    )
    expect(screen.getByText("Compacting context…")).toBeInTheDocument()
    expect(screen.getByText("Summarizing history")).toBeInTheDocument()
    expect(screen.queryByText("Context compacted")).not.toBeInTheDocument()
  })

  it("offers the summary the backend claimed", () => {
    renderParts([
      compactionPart(
        {
          contextCompaction: { version: 1 },
          [COMPACTION_SUMMARY_META_KEY]: true,
        },
        "We refactored the parser."
      ),
    ])
    expect(screen.getByText("Context compacted")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /Summary/ })).toBeInTheDocument()
  })

  it("does not offer a legacy call's metadata as a summary", () => {
    renderParts([
      compactionPart(
        { contextCompaction: { version: 1 } },
        '{"trigger":"manual","preTokens":191322,"postTokens":10086}'
      ),
    ])
    expect(screen.getByText("Context compacted")).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /Summary/ })
    ).not.toBeInTheDocument()
  })
})
