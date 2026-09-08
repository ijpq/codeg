import { cleanup, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, describe, expect, it, vi } from "vitest"
import enMessages from "@/i18n/messages/en.json"
import { ComposerCodexQuota } from "./composer-codex-quota"

const mocks = vi.hoisted(() => ({
  quota: vi.fn(),
}))

vi.mock("@/lib/api", () => ({
  codexQuotaSnapshot: mocks.quota,
}))

vi.mock("@/contexts/tab-context", () => ({
  useTabStore: (
    selector: (state: {
      tabs: Array<{
        id: string
        kind: "conversation"
        conversationId: number
      }>
    }) => unknown
  ) =>
    selector({
      tabs: [{ id: "tab-1", kind: "conversation", conversationId: 42 }],
    }),
}))

function renderQuota(agentType: "codex" | "claude" = "codex") {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <ComposerCodexQuota
        tabId="tab-1"
        agentType={agentType}
        isPrompting={false}
      />
    </NextIntlClientProvider>
  )
}

describe("ComposerCodexQuota", () => {
  afterEach(() => {
    cleanup()
    mocks.quota.mockReset()
  })

  it("shows the relay package label and weekly remaining allowance", async () => {
    mocks.quota.mockResolvedValue({
      planType: "pro",
      limitId: "codex",
      limitName: "PRO 50x",
      weekly: {
        usedPercent: 37,
        windowMinutes: 10080,
        resetsAt: 1787220000,
      },
      shortWindow: null,
      observedAt: "2026-08-20T10:00:00Z",
    })

    renderQuota()

    expect(
      await screen.findByRole("button", {
        name: "PRO 50x · 63% weekly left",
      })
    ).toBeInTheDocument()
    expect(mocks.quota).toHaveBeenCalledWith(42)
  })

  it("does not query or render allowance chrome for another agent", async () => {
    renderQuota("claude")
    await waitFor(() => expect(mocks.quota).not.toHaveBeenCalled())
    expect(screen.queryByRole("button")).not.toBeInTheDocument()
  })
})
