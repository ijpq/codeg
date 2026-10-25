"use client"

import { useEffect, useState } from "react"
import { getDeferredHistoryContent } from "@/lib/api"

/** Read large persisted output only when its own row is expanded. */
export function useDeferredHistory(
  reference: string | null | undefined,
  preview: string,
  open: boolean
) {
  const [resolved, setResolved] = useState<{
    reference: string
    content: string
  } | null>(null)
  useEffect(() => {
    if (!open || !reference || resolved?.reference === reference) return
    let current = true
    void getDeferredHistoryContent(reference)
      .then((result) => {
        if (current) setResolved({ reference, content: result.content })
      })
      .catch(() => undefined)
    return () => {
      current = false
    }
  }, [open, reference, resolved?.reference])
  return resolved && resolved.reference === reference
    ? resolved.content
    : preview
}
