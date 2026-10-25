import { describe, expect, it } from "vitest"
import {
  resolveConversationSessionId,
  isRetryableSessionRestoreConflict,
  shouldAwaitHistoricalSessionDetail,
} from "./session-restore"

describe("resolveConversationSessionId", () => {
  const sessions = [
    { agentType: "codex", externalId: "codex-original" },
    { agentType: "claude_code", externalId: "claude-original" },
  ]
  it("keeps a live fork/rollover ahead of stale polled history", () => {
    expect(
      resolveConversationSessionId({
        agent: "codex",
        sessions,
        runtimeId: "codex-new-fork",
        detailId: "codex-original",
      })
    ).toBe("codex-new-fork")
  })
  it("never resumes the other engine's runtime session after a handoff", () => {
    expect(
      resolveConversationSessionId({
        agent: "claude_code",
        sessions,
        runtimeId: "codex-original",
        detailId: "codex-original",
      })
    ).toBe("claude-original")
    expect(
      resolveConversationSessionId({
        agent: "codex",
        sessions,
        runtimeId: "claude-original",
        detailId: "claude-original",
      })
    ).toBe("codex-original")
  })
  it("retains the existing cold-open and unpersisted draft behavior", () => {
    expect(
      resolveConversationSessionId({
        agent: "codex",
        runtimeId: null,
        detailId: "native",
      })
    ).toBe("native")
    expect(
      resolveConversationSessionId({
        agent: "codex",
        runtimeId: "draft",
        detailId: null,
      })
    ).toBe("draft")
  })
})

describe("shouldAwaitHistoricalSessionDetail", () => {
  it("lets Codex restore run in parallel with persisted detail loading", () => {
    expect(
      shouldAwaitHistoricalSessionDetail({
        usesPersistedDetailIdentity: true,
        agentType: "codex",
        detailLoaded: false,
        detailLoading: false,
        detailError: null,
      })
    ).toBe(false)
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

  it("still gates agents that require detail-provided session identity", () => {
    expect(
      shouldAwaitHistoricalSessionDetail({
        usesPersistedDetailIdentity: true,
        agentType: "claude_code",
        detailLoaded: false,
        detailLoading: false,
        detailError: null,
      })
    ).toBe(true)
  })

  it("does not gate drafts, Codex, cline, or the existing detail-error path", () => {
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

describe("isRetryableSessionRestoreConflict", () => {
  it("recognizes writer/single-flight/session-mismatch races", () => {
    expect(
      isRetryableSessionRestoreConflict(
        new Error("thread abc already has an active writer")
      )
    ).toBe(true)
    expect(
      isRetryableSessionRestoreConflict(
        "conversation 7 restore is still in progress; retry"
      )
    ).toBe(true)
    expect(
      isRetryableSessionRestoreConflict(
        new Error("ACP session mismatch: expected S1, got S2")
      )
    ).toBe(true)
    expect(
      isRetryableSessionRestoreConflict(
        new Error("conversation 7 session changed during restore; retry")
      )
    ).toBe(true)
  })

  it("does not retry permanent load errors", () => {
    expect(
      isRetryableSessionRestoreConflict(new Error("session abc was not found"))
    ).toBe(false)
  })
})
