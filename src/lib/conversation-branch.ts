import type { ContentBlock, MessageTurn } from "@/lib/types"

export function visibleTurnText(blocks: ContentBlock[]): string {
  return blocks
    .filter(
      (block): block is Extract<ContentBlock, { type: "text" }> =>
        block.type === "text"
    )
    .map((block) => block.text.trim())
    .filter(Boolean)
    .join("\n")
}

/**
 * Build the explicit fallback passed when an ACP agent cannot fork natively.
 * It deliberately excludes thinking, tool input/output and internal events.
 */
export function buildConversationBranchSnapshot(
  turns: MessageTurn[],
  throughMessageId?: string | null,
  maxChars = 120_000
): string {
  const throughIndex = throughMessageId
    ? turns.findIndex((turn) => turn.id === throughMessageId)
    : turns.length - 1
  const bounded = turns.slice(
    0,
    throughIndex >= 0 ? throughIndex + 1 : turns.length
  )
  const entries = bounded
    .filter((turn) => turn.role === "user" || turn.role === "assistant")
    .map((turn) => {
      const text = visibleTurnText(turn.blocks)
      return text
        ? `${turn.role === "user" ? "User" : "Assistant"}:\n${text}`
        : ""
    })
    .filter(Boolean)

  const header = "Codeg conversation branch context snapshot\n"
  const kept: string[] = []
  let used = header.length
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry = entries[index]
    if (used + entry.length + 2 > maxChars && kept.length > 0) break
    kept.unshift(entry)
    used += entry.length + 2
  }
  return `${header}${kept.join("\n\n")}`
}

export function latestAssistantConclusion(turns: MessageTurn[]): string {
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    if (turns[index].role !== "assistant") continue
    const text = visibleTurnText(turns[index].blocks)
    if (text) return text
  }
  return ""
}
