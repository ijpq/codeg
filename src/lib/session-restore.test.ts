import { describe, expect, it } from "vitest"
import { shouldAwaitHistoricalSessionDetail } from "./session-restore"

describe("shouldAwaitHistoricalSessionDetail", () => {
  it("blocks the empty non-loading commit before persisted detail resolves", () => {
    expect(
      shouldAwaitHistoricalSessionDetail({
        usesPersistedDetailIdentity: true,
        agentType: "codex",
        detailLoaded: false,
        detailLoading: false,
        detailError: null,
      })
    ).toBe(true)
  })

  it("unblocks only after authoritative detail resolves", () => {
    expect(
      shouldAwaitHistoricalSessionDetail({
        usesPersistedDetailIdentity: true,
        agentType: "codex",
        detailLoaded: true,
        detailLoading: false,
        detailError: null,
      })
    ).toBe(false)
  })

  it("does not gate drafts, cline, or the existing detail-error path", () => {
    const base = {
      usesPersistedDetailIdentity: true,
      agentType: "codex",
      detailLoaded: false,
      detailLoading: false,
      detailError: null,
    }
    expect(
      shouldAwaitHistoricalSessionDetail({
        ...base,
        usesPersistedDetailIdentity: false,
      })
    ).toBe(false)
    expect(
      shouldAwaitHistoricalSessionDetail({ ...base, agentType: "cline" })
    ).toBe(false)
    expect(
      shouldAwaitHistoricalSessionDetail({
        ...base,
        detailError: "load failed",
      })
    ).toBe(false)
  })
})
