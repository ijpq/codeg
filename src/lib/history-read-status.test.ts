import { describe, expect, it } from "vitest"
import { historyReadStatus } from "./history-read-status"
import type { ConversationHistoryPage } from "@/lib/types"

describe("partial history status", () => {
  it.each([
    ["index_building", "partialHistoryBuilding"],
    ["index_rebuilding", "partialHistoryRebuilding"],
    ["index_capacity_limited", "partialHistoryIndexLimited"],
    ["record_boundary_unavailable", "partialHistoryBudget"],
    ["HISTORY_PARSE_FAILED: invalid record", "partialHistoryParseFailed"],
    [undefined, "partialHistoryBudget"],
  ])("reports %s without claiming that the index is building", (issue, key) => {
    expect(
      historyReadStatus({
        page_complete: false,
        index_status: "ready",
        read_issue: issue,
      } as ConversationHistoryPage)
    ).toBe(key)
  })
})
