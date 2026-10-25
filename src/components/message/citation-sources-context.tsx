"use client"

import { createContext, useContext, useMemo, type ReactNode } from "react"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import { extractCitationSources, type CitationSource } from "@/lib/citations"

export const CitationSourcesContext = createContext<readonly CitationSource[]>([])

export function CitationSourcesScope({
  parts,
  children,
}: {
  parts: AdaptedContentPart[]
  children: ReactNode
}) {
  const parent = useContext(CitationSourcesContext)
  const sources = useMemo(() => {
    const own = extractCitationSources(parts)
    return own.length > 0 ? [...parent, ...own] : parent
  }, [parent, parts])
  return (
    <CitationSourcesContext.Provider value={sources}>
      {children}
    </CitationSourcesContext.Provider>
  )
}
