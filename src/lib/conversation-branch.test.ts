import { describe, expect, it } from "vitest"
import { latestAssistantConclusion } from "./conversation-branch"
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

describe("latestAssistantConclusion", () => {
  it("uses user-visible assistant text only", () => {
    const conclusion = latestAssistantConclusion([
      turn("u1", "user", "one"),
      turn("a1", "assistant", "two"),
      turn("u2", "user", "three"),
    ])
    expect(conclusion).toBe("two")
  })
})
