"use client"

import { useEffect, useState } from "react"
import type { UserImageDisplay } from "@/lib/adapters/ai-elements-adapter"
import { getDeferredHistoryContent } from "@/lib/api"

interface HistoryImageLoadState {
  reference: string
  image: UserImageDisplay | null
  failed: boolean
}

/**
 * Resolve large historical image bytes only while their virtualized message is
 * mounted. The bounded conversation response keeps just an authenticated
 * opaque reference; scrolling the image into the rendered window fetches the
 * exact original bytes and verifies their transcript hash on the server.
 */
export function useResolvedHistoryImage(image: UserImageDisplay | null): {
  image: UserImageDisplay | null
  loading: boolean
  failed: boolean
} {
  const reference = image?.deferredRef ?? null
  const imageName = image?.name ?? "image"
  const imageMimeType = image?.mime_type ?? "image/png"
  const imageUri = image?.uri ?? null
  const [load, setLoad] = useState<HistoryImageLoadState | null>(null)

  useEffect(() => {
    if (!reference) return

    let active = true
    void getDeferredHistoryContent(reference)
      .then((result) => {
        if (!active) return
        setLoad({
          reference,
          image: {
            name: imageName,
            data: result.content,
            mime_type: result.mime_type ?? imageMimeType,
            uri: imageUri,
            deferredRef: null,
          },
          failed: false,
        })
      })
      .catch(() => {
        if (active) setLoad({ reference, image: null, failed: true })
      })

    return () => {
      active = false
    }
  }, [imageMimeType, imageName, imageUri, reference])

  if (!reference) return { image, loading: false, failed: false }
  if (load?.reference !== reference) {
    return { image: null, loading: true, failed: false }
  }
  return { image: load.image, loading: false, failed: load.failed }
}
