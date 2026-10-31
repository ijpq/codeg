import type { ConversationHistoryPage } from "@/lib/types"

// A byte-limited page and an incomplete index are different states. The
// server owns the reason; never infer indexing merely from page_complete.
export function historyReadStatus(page: ConversationHistoryPage) {
  switch (page.read_issue) {
    case "index_building":
      return "partialHistoryBuilding"
    case "index_rebuilding":
      return "partialHistoryRebuilding"
    case "index_capacity_limited":
      return "partialHistoryIndexLimited"
    default:
      return page.read_issue?.includes("HISTORY_PARSE_FAILED")
        ? "partialHistoryParseFailed"
        : "partialHistoryBudget"
  }
}
