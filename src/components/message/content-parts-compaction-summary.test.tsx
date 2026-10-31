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
import type {
  AdaptedContentPart,
  AdaptedToolCallPart,
} from "@/lib/adapters/ai-elements-adapter"
import { COMPACTION_SUMMARY_META_KEY } from "@/lib/context-compaction"
import { ContextCompactionCard } from "./context-compaction-card"
import {
  dedupeCompactionItems,
  type ThreadRenderItem,
} from "./message-list-view"

function compactionPart(
  meta: Record<string, unknown>,
  output: string
): AdaptedToolCallPart {
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
  it("settles one card when a restored start overlaps the native completion in the continuing reply", () => {
    const start = {
      contextCompaction: {
        version: 1,
        eventId: "active-operation",
        status: "in_progress",
      },
    }
    const completed = {
      contextCompaction: {
        version: 1,
        eventId: "native-entry",
        status: "completed",
      },
    }
    const reply: Extract<ThreadRenderItem, { kind: "turn" }> = {
      key: "reply",
      kind: "turn",
      phase: "streaming",
      group: {
        id: "reply",
        role: "assistant",
        images: [],
        resources: [],
        parts: [
          {
            ...compactionPart(completed, "summary"),
            toolCallId: "active-operation",
          },
          {
            type: "text",
            text: "The reply continues after the completed compaction.",
          },
        ],
      },
      isResponseComplete: false,
      showStats: false,
      isRoleTransition: false,
      previousUserIndex: null,
      previousUserId: null,
      isLastAssistantRun: false,
      isThreadTail: false,
      sourceTurns: [],
    }
    const items = dedupeCompactionItems([
      {
        key: "restored-start",
        kind: "compaction",
        meta: start,
        callId: "active-operation",
      },
      reply,
    ])
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        {items.map((item) =>
          item.kind === "compaction" ? (
            <ContextCompactionCard key={item.key} meta={item.meta} />
          ) : item.kind === "turn" ? (
            <ContentPartsRenderer
              key={item.key}
              parts={item.group.parts}
              role="assistant"
            />
          ) : null
        )}
      </NextIntlClientProvider>
    )
    expect(screen.getAllByTestId("context-compaction")).toHaveLength(1)
    expect(screen.getByTestId("context-compaction")).toHaveAttribute(
      "data-compaction-status",
      "completed"
    )
    expect(screen.getByTestId("context-compaction")).toHaveAttribute(
      "data-compaction-event-id",
      "native-entry"
    )
    expect(screen.queryByText("Compacting context…")).not.toBeInTheDocument()
    expect(
      screen.getByText("The reply continues after the completed compaction.")
    ).toBeInTheDocument()
  })

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
