import { describe, expect, it } from "vitest"
import { buildConversationBranchSnapshot } from "./conversation-branch"
import type { MessageTurn } from "./types"

const turn = (
  id: string,
  role: MessageTurn["role"],
  text: string
): MessageTurn => ({
  id,
  role,
  blocks: [
    { type: "text", text },
    { type: "thinking", text: "private reasoning" },
  ],
  timestamp: "2026-08-13T00:00:00Z",
})

describe("buildConversationBranchSnapshot", () => {
  it("uses only user-visible text through the selected message", () => {
    const snapshot = buildConversationBranchSnapshot(
      [
        turn("u1", "user", "one"),
        turn("a1", "assistant", "two"),
        turn("u2", "user", "three"),
      ],
      "a1"
    )
    expect(snapshot).toContain("User:\none")
    expect(snapshot).toContain("Assistant:\ntwo")
    expect(snapshot).not.toContain("three")
    expect(snapshot).not.toContain("private reasoning")
  })
})
