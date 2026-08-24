import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"

export interface CitationSource {
  reference_id: string
  url: string
  title: string
  site_name: string
}

const MARKER_RE = /\uE200cite\uE202([^\uE201]+)\uE201/g

function safeHttpUrl(raw: unknown): string | null {
  if (typeof raw !== "string") return null
  try {
    const parsed = new URL(raw)
    return parsed.protocol === "http:" || parsed.protocol === "https:"
      ? parsed.toString()
      : null
  } catch {
    return null
  }
}

function sourcesFromMeta(meta: Record<string, unknown> | null | undefined) {
  const raw = meta?.["codeg.citations"]
  if (!Array.isArray(raw)) return []
  const sources: CitationSource[] = []
  for (const item of raw) {
    if (!item || typeof item !== "object" || Array.isArray(item)) continue
    const value = item as Record<string, unknown>
    const referenceId = value.reference_id
    const url = safeHttpUrl(value.url)
    if (typeof referenceId !== "string" || !referenceId.trim() || !url) {
      continue
    }
    const parsed = new URL(url)
    sources.push({
      reference_id: referenceId,
      url,
      title:
        typeof value.title === "string" && value.title.trim()
          ? value.title.trim()
          : parsed.hostname,
      site_name:
        typeof value.site_name === "string" && value.site_name.trim()
          ? value.site_name.trim()
          : parsed.hostname,
    })
  }
  return sources
}

function collectPartSources(
  parts: readonly AdaptedContentPart[],
  output: CitationSource[]
): void {
  for (const part of parts) {
    if (part.type === "tool-call") {
      output.push(...sourcesFromMeta(part.meta))
    } else if (part.type === "tool-group") {
      for (const item of part.items) output.push(...sourcesFromMeta(item.meta))
    } else if (part.type === "goal-run") {
      collectPartSources([part.start, ...part.items], output)
      if (part.end) collectPartSources([part.end], output)
    }
  }
}

export function extractCitationSources(
  parts: readonly AdaptedContentPart[]
): CitationSource[] {
  const collected: CitationSource[] = []
  collectPartSources(parts, collected)
  const byReference = new Map<string, CitationSource>()
  for (const source of collected) {
    if (!byReference.has(source.reference_id)) {
      byReference.set(source.reference_id, source)
    }
  }
  return [...byReference.values()]
}

function citationIndex(sources: readonly CitationSource[]) {
  const byReference = new Map<string, CitationSource>()
  const numberByUrl = new Map<string, number>()
  const numbered: CitationSource[] = []
  for (const source of sources) {
    byReference.set(source.reference_id, source)
    if (!numberByUrl.has(source.url)) {
      numbered.push(source)
      numberByUrl.set(source.url, numbered.length)
    }
  }
  return { byReference, numberByUrl, numbered }
}

function markerSources(
  encodedIds: string,
  byReference: ReadonlyMap<string, CitationSource>
) {
  const resolved: CitationSource[] = []
  let unresolved = false
  for (const id of encodedIds
    .split("\uE202")
    .map((value) => value.trim())
    .filter(Boolean)) {
    const source = byReference.get(id)
    if (source) resolved.push(source)
    else unresolved = true
  }
  return { resolved, unresolved }
}

function markdownTitle(source: CitationSource): string {
  return `${source.title}${source.site_name ? ` — ${source.site_name}` : ""}`
    .replace(/\\/g, "\\\\")
    .replace(/"/g, '\\"')
    .replace(/[\r\n]+/g, " ")
}

export function renderCitationMarkdown(
  text: string,
  sources: readonly CitationSource[]
): string {
  const { byReference, numberByUrl } = citationIndex(sources)
  return text.replace(MARKER_RE, (_marker, encodedIds: string) => {
    const { resolved, unresolved } = markerSources(encodedIds, byReference)
    const seen = new Set<string>()
    const links = resolved
      .filter((source) => !seen.has(source.url) && seen.add(source.url))
      .map(
        (source) =>
          `[${numberByUrl.get(source.url)}](<${source.url}> "${markdownTitle(source)}")`
      )
      .join("")
    const fallback =
      unresolved || resolved.length === 0 ? "［引用来源暂不可解析］" : ""
    return `${fallback}${links}`
  })
}

export function renderCitationPlainText(
  text: string,
  sources: readonly CitationSource[]
): string {
  const { byReference, numberByUrl, numbered } = citationIndex(sources)
  const used = new Set<string>()
  const rendered = text.replace(MARKER_RE, (_marker, encodedIds: string) => {
    const { resolved, unresolved } = markerSources(encodedIds, byReference)
    const labels = new Set<number>()
    for (const source of resolved) {
      used.add(source.url)
      const number = numberByUrl.get(source.url)
      if (number) labels.add(number)
    }
    return `${unresolved || resolved.length === 0 ? "［引用来源暂不可解析］" : ""}${[
      ...labels,
    ]
      .map((number) => `[${number}]`)
      .join("")}`
  })
  const usedSources = numbered.filter((source) => used.has(source.url))
  if (usedSources.length === 0) return rendered
  return `${rendered}\n\n来源：\n${usedSources
    .map(
      (source) =>
        `[${numberByUrl.get(source.url)}] ${source.title}：${source.url}`
    )
    .join("\n")}`
}
