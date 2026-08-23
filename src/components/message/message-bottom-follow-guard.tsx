"use client"

import { memo, useCallback, useEffect, useRef } from "react"
import { useStickToBottomContext } from "use-stick-to-bottom"

/**
 * Browser scroll positions can be fractional at non-100% zoom / Windows DPI,
 * while `scrollTop` is sometimes rounded by the engine. Keep a small tolerance
 * so an already exact viewport is not written on every observer callback.
 */
const EXACT_BOTTOM_EPSILON_PX = 0.5

export function pinViewportToExactBottom(viewport: HTMLElement): boolean {
  const target = Math.max(0, viewport.scrollHeight - viewport.clientHeight)
  if (Math.abs(target - viewport.scrollTop) <= EXACT_BOTTOM_EPSILON_PX) {
    return false
  }
  viewport.scrollTop = target
  return true
}

/**
 * `use-stick-to-bottom` observes message-content height, but not the scroll
 * viewport itself. The latter changes whenever the docked composer grows,
 * status/permission rows mount, browser zoom changes, or Windows DPI rounding
 * settles. In those cases the old scroll offset can remain a few pixels short
 * even though the user never left follow mode.
 *
 * Observe BOTH surfaces and perform a double-rAF correction after layout has
 * stabilised. Repeated streaming resizes are coalesced, so this does not add a
 * synchronous scroll per token. `escapedFromLock` is the library's durable
 * signal that the user deliberately scrolled upward; it is checked again in
 * each frame so a late callback never drags a reader back to the bottom.
 */
export const MessageBottomFollowGuard = memo(function MessageBottomFollowGuard({
  layoutSignal,
}: {
  /** Turn-complete / deliverable-mount convergence signal. */
  layoutSignal: string
}) {
  const { scrollRef, contentRef, scrollToBottom, state } =
    useStickToBottomContext()
  const firstFrameRef = useRef<number | null>(null)
  const finalFrameRef = useRef<number | null>(null)
  const disposedRef = useRef(false)

  const cancelScheduled = useCallback(() => {
    if (firstFrameRef.current !== null) {
      cancelAnimationFrame(firstFrameRef.current)
      firstFrameRef.current = null
    }
    if (finalFrameRef.current !== null) {
      cancelAnimationFrame(finalFrameRef.current)
      finalFrameRef.current = null
    }
  }, [])

  const scheduleCorrection = useCallback(() => {
    if (disposedRef.current || state.escapedFromLock) return
    cancelScheduled()
    firstFrameRef.current = requestAnimationFrame(() => {
      firstFrameRef.current = null
      if (disposedRef.current || state.escapedFromLock) return
      // Let the library update its own lock/animation bookkeeping first.
      void scrollToBottom({ animation: "instant", ignoreEscapes: true })
      finalFrameRef.current = requestAnimationFrame(() => {
        finalFrameRef.current = null
        if (disposedRef.current || state.escapedFromLock) return
        const viewport = scrollRef.current
        if (viewport) pinViewportToExactBottom(viewport)
      })
    })
  }, [cancelScheduled, scrollRef, scrollToBottom, state])

  useEffect(() => {
    disposedRef.current = false
    const viewport = scrollRef.current
    const content = contentRef.current
    if (!viewport || !content) return

    const observer = new ResizeObserver(scheduleCorrection)
    observer.observe(viewport)
    observer.observe(content)
    scheduleCorrection()

    // Webfonts can change the final Chinese line height without a React
    // render. ResizeObserver normally catches it; this readiness callback is
    // an inexpensive final backstop for browser implementations that batch
    // the text reflow and virtualizer measurement in different frames.
    void document.fonts?.ready.then(() => {
      if (!disposedRef.current) scheduleCorrection()
    })

    return () => {
      disposedRef.current = true
      observer.disconnect()
      cancelScheduled()
    }
  }, [cancelScheduled, contentRef, scheduleCorrection, scrollRef])

  useEffect(() => {
    // A live row can be replaced by its persisted row with the same measured
    // height, so no ResizeObserver notification is guaranteed. Turn status
    // and deliverable count flow through this signal to force convergence.
    scheduleCorrection()
  }, [layoutSignal, scheduleCorrection])

  return null
})
