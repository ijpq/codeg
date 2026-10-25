import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import messages from "@/i18n/messages/en.json"
import type { useBusinessConversation } from "@/hooks/use-business-conversation"
import { supportsBusinessHandoff } from "@/lib/business-conversation"
import { BusinessAgentControl } from "./business-agent-control"

describe("business main Agent selector", () => {
  it("keeps all three engines selectable while Pi is active", async () => {
    const business: ReturnType<typeof useBusinessConversation> = {
      state: null,
      pending: false,
      error: null,
      switchAgent: vi.fn(async () => {}),
      refresh: vi.fn(async () => {}),
    }
    render(
      <NextIntlClientProvider locale="en" messages={messages}>
        <BusinessAgentControl
          conversationId={1}
          folderId={2}
          agent="pi"
          business={business}
        />
      </NextIntlClientProvider>
    )
    const selector = screen.getByRole("combobox")
    expect(supportsBusinessHandoff("pi")).toBe(true)
    expect(selector).toHaveValue("pi")
    expect(
      screen.getAllByRole("option").map((item) => item.textContent)
    ).toEqual(["Codex", "Claude Code", "Pi"])
    await userEvent.selectOptions(selector, "codex")
    expect(business.switchAgent).toHaveBeenCalledWith("codex")
    await userEvent.selectOptions(selector, "claude_code")
    expect(business.switchAgent).toHaveBeenCalledWith("claude_code")
  })
})
