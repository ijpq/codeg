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

export function latestAssistantConclusion(turns: MessageTurn[]): string {
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    if (turns[index].role !== "assistant") continue
    const text = visibleTurnText(turns[index].blocks)
    if (text) return text
  }
  return ""
}
