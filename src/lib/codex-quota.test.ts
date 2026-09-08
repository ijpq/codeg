import { describe, expect, it } from "vitest"
import {
  codexPlanLabel,
  formatQuotaPercent,
  quotaRemainingPercent,
} from "@/lib/codex-quota"
import type { CodexQuotaSnapshot } from "@/lib/types"

function snapshot(overrides: Partial<CodexQuotaSnapshot> = {}) {
  return {
    planType: "pro",
    limitId: "codex",
    limitName: null,
    weekly: null,
    shortWindow: null,
    observedAt: null,
    ...overrides,
  } satisfies CodexQuotaSnapshot
}

describe("codex quota display", () => {
  it("prefers the relay package label without exposing an account", () => {
    expect(codexPlanLabel(snapshot({ limitName: "PRO 50x" }))).toBe("PRO 50x")
  })

  it("falls back to stable Codex plan labels", () => {
    expect(codexPlanLabel(snapshot({ planType: "plus" }))).toBe("Plus")
    expect(codexPlanLabel(snapshot({ planType: "prolite" }))).toBe("Pro Lite")
    expect(codexPlanLabel(snapshot({ planType: "pro_20x" }))).toBe("Pro 20x")
  })

  it("clamps and formats remaining percent", () => {
    expect(quotaRemainingPercent(74)).toBe(26)
    expect(quotaRemainingPercent(-5)).toBe(100)
    expect(quotaRemainingPercent(120)).toBe(0)
    expect(formatQuotaPercent(26.25)).toBe("26.3%")
  })
})
