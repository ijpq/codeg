import { describe, expect, it } from "vitest"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import {
  extractCitationSources,
  renderCitationMarkdown,
  renderCitationPlainText,
} from "./citations"

const sourceA = {
  citation_id: "turn0search0",
  url: "https://example.com/a?q=%E4%B8%AD%E6%96%87&x=1",
  title: "来源 A",
  domain: "example.com",
  source_type: "text_result",
}
const sourceB = {
  citation_id: "turn0search1",
  url: "https://docs.example.org/b#part",
  title: "来源 B",
  domain: "docs.example.org",
  source_type: "text_result",
}

describe("Codex citation rendering", () => {
  it("renders a source as a numbered safe Markdown link", () => {
    expect(
      renderCitationMarkdown("结论。\uE200cite\uE202turn0search0\uE201", [
        sourceA,
      ])
    ).toContain(`[1](<${sourceA.url}> "来源 A — example.com")`)
  })

  it("supports multiple sources at one position and reuses source numbers", () => {
    const rendered = renderCitationMarkdown(
      "A\uE200cite\uE202turn0search0\uE202turn0search1\uE201 B\uE200cite\uE202turn0search0\uE201",
      [sourceA, sourceB]
    )
    expect(rendered.match(/\[1\]/g)).toHaveLength(2)
    expect(rendered.match(/\[2\]/g)).toHaveLength(1)
  })

  it.each([
    "段落末尾\uE200cite\uE202turn0search0\uE201",
    "- 列表\uE200cite\uE202turn0search0\uE201",
    "| 表格 |\n| --- |\n| 值\uE200cite\uE202turn0search0\uE201 |",
    "## 标题\uE200cite\uE202turn0search0\uE201",
    "**加粗结论\uE200cite\uE202turn0search0\uE201**",
  ])("preserves surrounding Markdown: %s", (text) => {
    expect(renderCitationMarkdown(text, [sourceA])).not.toContain("\uE200cite")
  })

  it("keeps unmapped legacy citations visibly unresolved", () => {
    expect(
      renderCitationMarkdown("旧回答\uE200cite\uE202turn9view0\uE201", [])
    ).toBe("旧回答*来源缺失*")
  })

  it("rejects malicious URLs from tool metadata", () => {
    const parts: AdaptedContentPart[] = [
      {
        type: "tool-call",
        toolCallId: "ws",
        toolName: "web_search",
        input: null,
        state: "output-available",
        meta: {
          "codeg.citations": [{ ...sourceA, url: "javascript:alert(1)" }],
        },
      },
    ]
    expect(extractCitationSources(parts)).toEqual([])
  })

  it.each(["javascript:alert(1)", "file:///etc/passwd", "data:text/plain,x"])(
    "rejects dangerous citation URL %s",
    (url) => {
      const parts: AdaptedContentPart[] = [
        {
          type: "tool-call",
          toolCallId: "ws",
          toolName: "web_search",
          input: null,
          state: "output-available",
          meta: { "codeg.citations": [{ ...sourceA, url }] },
        },
      ]
      expect(extractCitationSources(parts)).toEqual([])
    }
  )

  it("plain text replaces private ids and appends each used URL once", () => {
    const rendered = renderCitationPlainText(
      "A\uE200cite\uE202turn0search0\uE201 B\uE200cite\uE202turn0search0\uE201",
      [sourceA]
    )
    expect(rendered).not.toContain("\uE200cite")
    expect(rendered.match(/https:\/\/example\.com/g)).toHaveLength(1)
  })

  it("does not alter ordinary Markdown links", () => {
    const markdown = "[普通链接](https://example.net/path)"
    expect(renderCitationMarkdown(markdown, [])).toBe(markdown)
  })

  it("reuses one source number when different citation ids share a URL", () => {
    const rendered = renderCitationMarkdown(
      "A\uE200cite\uE202turn0search0\uE201 B\uE200cite\uE202turn4view9\uE201",
      [sourceA, { ...sourceA, citation_id: "turn4view9" }]
    )
    expect(rendered.match(/\[1\]/g)).toHaveLength(2)
    expect(rendered).not.toContain("[2]")
  })

  it("keeps encoded Chinese parameters and long https URLs", () => {
    const longUrl = `https://example.cn/%E8%B5%84%E6%96%99?q=${"x".repeat(1024)}`
    const source = {
      ...sourceA,
      url: longUrl,
      title: "中文来源标题",
      domain: "example.cn",
    }
    expect(
      renderCitationMarkdown("结论\uE200cite\uE202turn0search0\uE201", [source])
    ).toContain(`<${longUrl}>`)
  })

  it("extracts sources through grouped tool parts used by merged turns", () => {
    const parts: AdaptedContentPart[] = [
      {
        type: "tool-group",
        isStreaming: false,
        items: [
          {
            type: "tool-call",
            toolCallId: "ws",
            toolName: "web_search",
            input: null,
            state: "output-available",
            meta: { "codeg.citations": [sourceA, sourceB] },
          },
        ],
      },
    ]
    expect(extractCitationSources(parts)).toEqual([sourceA, sourceB])
  })

  it("reads both the new structured schema and legacy fix1 metadata", () => {
    const parts: AdaptedContentPart[] = [
      {
        type: "tool-call",
        toolCallId: "ws",
        toolName: "web_search",
        input: null,
        state: "output-available",
        meta: {
          "codeg.citations": [
            {
              reference_id: "turn2view0",
              url: "https://legacy.example/path",
              title: "旧来源",
              site_name: "legacy.example",
            },
          ],
        },
      },
    ]
    expect(extractCitationSources(parts)[0]).toMatchObject({
      citation_id: "turn2view0",
      domain: "legacy.example",
      source_type: "web_search",
    })
  })
})
