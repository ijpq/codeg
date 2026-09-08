"use client"

import { memo, useCallback, useEffect, useRef } from "react"
import { useStickToBottomContext } from "use-stick-to-bottom"

/** Fractional scroll/DOM geometry is normal at browser zoom and Windows DPI. */
const GEOMETRY_EPSILON_PX = 1
const NEAR_BOTTOM_THRESHOLD_PX = 80
const SENTINEL_VISUAL_GAP_PX = 12

export interface MessageBottomGeometry {
  dockHeight: number
  dockOverlap: number
  virtualOverflowCorrection: number
  sentinelGap: number
  sentinelFullyVisible: boolean
}

function finiteNonNegative(value: number): number {
  return Number.isFinite(value) ? Math.max(0, value) : 0
}

/**
 * Measure the three independent surfaces that define the real transcript end:
 * the scroll viewport, the final rendered virtual row and the composer dock.
 *
 * `virtua` can briefly retain its previous estimate while Markdown, an image,
 * a tool card or a webfont changes the final row. In that state `scrollTop ===
 * scrollHeight - clientHeight` is true even though the row overflows the
 * virtualizer's declared end. The correction below is derived from DOM rects,
 * not a guessed line-height or composer size.
 */
export function measureMessageBottomGeometry(
  viewport: HTMLElement,
  content: HTMLElement,
  sentinel: HTMLElement,
  lastRow: HTMLElement | null,
  bottomDock: HTMLElement | null
): MessageBottomGeometry {
  const viewportRect = viewport.getBoundingClientRect()
  const sentinelRect = sentinel.getBoundingClientRect()
  const dockRect = bottomDock?.getBoundingClientRect() ?? null
  const currentCorrection = finiteNonNegative(
    Number.parseFloat(getComputedStyle(sentinel).marginTop)
  )

  // Remove the correction already applied to recover the virtualizer's raw
  // declared end, then compare that boundary with the real final row.
  const rawSentinelTop = sentinelRect.top - currentCorrection
  const virtualOverflowCorrection = lastRow
    ? finiteNonNegative(lastRow.getBoundingClientRect().bottom - rawSentinelTop)
    : currentCorrection

  const dockIntersectsViewport = Boolean(
    dockRect &&
    dockRect.bottom > viewportRect.top &&
    dockRect.top < viewportRect.bottom
  )
  const dockOverlap = dockIntersectsViewport
    ? finiteNonNegative(
        viewportRect.bottom - (dockRect?.top ?? viewportRect.bottom)
      )
    : 0
  const dockHeight = finiteNonNegative(dockRect?.height ?? 0)
  const safeBottom = viewportRect.bottom - dockOverlap - SENTINEL_VISUAL_GAP_PX
  const sentinelGap = safeBottom - sentinelRect.bottom

  // Keep the content argument explicit: callers and tests verify that the
  // sentinel belongs to the currently mounted content generation.
  const sentinelBelongsToContent = content.contains(sentinel)

  return {
    dockHeight,
    dockOverlap,
    virtualOverflowCorrection,
    sentinelGap,
    sentinelFullyVisible:
      sentinelBelongsToContent && sentinelGap >= -GEOMETRY_EPSILON_PX,
  }
}

export function pinViewportToExactBottom(viewport: HTMLElement): boolean {
  const target = Math.max(0, viewport.scrollHeight - viewport.clientHeight)
  if (Math.abs(target - viewport.scrollTop) <= GEOMETRY_EPSILON_PX) {
    return false
  }
  viewport.scrollTop = target
  return true
}

function setPixelCustomProperty(
  target: HTMLElement,
  name: string,
  value: number
): boolean {
  const next = `${finiteNonNegative(value)}px`
  if (target.style.getPropertyValue(name) === next) return false
  target.style.setProperty(name, next)
  return true
}

/**
 * Geometry-based bottom following for the real CodeG transcript.
 *
 * All ResizeObserver, streaming, dock and turn-completion notifications are
 * coalesced into one requestAnimationFrame. A generation token invalidates old
 * callbacks on conversation switches. The guard always keeps geometry current,
 * but scrolls only while the reader remains in follow mode.
 */
export const MessageBottomFollowGuard = memo(function MessageBottomFollowGuard({
  layoutSignal,
  scopeKey,
}: {
  /** Turn-complete / tool / deliverable convergence signal. */
  layoutSignal: string
  /** Conversation generation; delayed work from another conversation is stale. */
  scopeKey?: string | number
}) {
  const { scrollRef, contentRef, state } = useStickToBottomContext()
  const frameRef = useRef<number | null>(null)
  const generationRef = useRef(0)
  const followingRef = useRef(true)
  const previousScrollTopRef = useRef(0)
  const lastRowRef = useRef<HTMLElement | null>(null)
  const scheduleRef = useRef<() => void>(() => undefined)

  const cancelScheduled = useCallback(() => {
    if (frameRef.current !== null) {
      cancelAnimationFrame(frameRef.current)
      frameRef.current = null
    }
  }, [])

  const scheduleCorrection = useCallback(() => {
    if (frameRef.current !== null) return
    const generation = generationRef.current
    frameRef.current = requestAnimationFrame(() => {
      frameRef.current = null
      if (generation !== generationRef.current) return

      const viewport = scrollRef.current
      const content = contentRef.current
      const sentinel = content?.querySelector<HTMLElement>(
        "[data-message-end-sentinel]"
      )
      if (!viewport || !content || !sentinel) return

      const shell = viewport.closest<HTMLElement>("[data-conversation-shell]")
      const bottomDock =
        shell?.querySelector<HTMLElement>(
          ":scope > [data-conversation-bottom-dock]"
        ) ?? null
      const lastRow = content.querySelector<HTMLElement>(
        '[data-message-thread-last="true"]'
      )
      lastRowRef.current = lastRow

      const geometry = measureMessageBottomGeometry(
        viewport,
        content,
        sentinel,
        lastRow,
        bottomDock
      )
      const overflowChanged = setPixelCustomProperty(
        content,
        "--codeg-message-virtual-overflow-correction",
        geometry.virtualOverflowCorrection
      )
      const overlapChanged = setPixelCustomProperty(
        content,
        "--codeg-conversation-bottom-dock-overlap",
        geometry.dockOverlap
      )
      const geometryChanged = overflowChanged || overlapChanged

      viewport.dataset.bottomFollowing = String(followingRef.current)
      viewport.dataset.bottomSentinelVisible = String(
        geometry.sentinelFullyVisible
      )
      viewport.dataset.bottomDockHeight = String(geometry.dockHeight)

      if (geometryChanged) {
        // CSS margins alter scrollHeight after this frame. Re-measure the
        // committed layout rather than using stale rects from above.
        scheduleRef.current()
        return
      }

      if (!followingRef.current || state.escapedFromLock) return
      const moved = pinViewportToExactBottom(viewport)
      if (moved || !geometry.sentinelFullyVisible) {
        // One converging follow-up handles fractional layout, virtua's row
        // remeasurement and browser font/image layout without fixed delays.
        scheduleRef.current()
      }
    })
  }, [contentRef, scrollRef, state])

  useEffect(() => {
    scheduleRef.current = scheduleCorrection
    return () => {
      if (scheduleRef.current === scheduleCorrection) {
        scheduleRef.current = () => undefined
      }
    }
  }, [scheduleCorrection])

  useEffect(() => {
    generationRef.current += 1
    const generation = generationRef.current
    followingRef.current = true
    cancelScheduled()

    const viewport = scrollRef.current
    const content = contentRef.current
    if (!viewport || !content) return

    previousScrollTopRef.current = viewport.scrollTop
    const shell = viewport.closest<HTMLElement>("[data-conversation-shell]")
    const bottomDock =
      shell?.querySelector<HTMLElement>(
        ":scope > [data-conversation-bottom-dock]"
      ) ?? null

    const resizeObserver = new ResizeObserver(() => scheduleCorrection())
    resizeObserver.observe(viewport)
    resizeObserver.observe(content)
    if (bottomDock) resizeObserver.observe(bottomDock)

    const bindDynamicTargets = () => {
      const sentinel = content.querySelector<HTMLElement>(
        "[data-message-end-sentinel]"
      )
      const lastRow = content.querySelector<HTMLElement>(
        '[data-message-thread-last="true"]'
      )
      if (sentinel) resizeObserver.observe(sentinel)
      if (lastRow && lastRow !== lastRowRef.current) {
        if (lastRowRef.current) resizeObserver.unobserve(lastRowRef.current)
        lastRowRef.current = lastRow
        resizeObserver.observe(lastRow)
      }
      scheduleCorrection()
    }

    const mutationObserver = new MutationObserver(bindDynamicTargets)
    mutationObserver.observe(content, { childList: true, subtree: true })
    bindDynamicTargets()

    const onScroll = () => {
      const current = viewport.scrollTop
      const remaining = Math.max(
        0,
        viewport.scrollHeight - viewport.clientHeight - current
      )
      if (
        current < previousScrollTopRef.current - GEOMETRY_EPSILON_PX &&
        remaining > NEAR_BOTTOM_THRESHOLD_PX
      ) {
        followingRef.current = false
      } else if (remaining <= NEAR_BOTTOM_THRESHOLD_PX) {
        followingRef.current = true
      }
      previousScrollTopRef.current = current
      viewport.dataset.bottomFollowing = String(followingRef.current)
    }
    viewport.addEventListener("scroll", onScroll, { passive: true })

    const onViewportResize = () => scheduleCorrection()
    window.addEventListener("resize", onViewportResize)
    window.visualViewport?.addEventListener("resize", onViewportResize)
    void document.fonts?.ready.then(() => {
      if (generation === generationRef.current) scheduleCorrection()
    })

    scheduleCorrection()
    return () => {
      generationRef.current += 1
      mutationObserver.disconnect()
      resizeObserver.disconnect()
      viewport.removeEventListener("scroll", onScroll)
      window.removeEventListener("resize", onViewportResize)
      window.visualViewport?.removeEventListener("resize", onViewportResize)
      content.style.removeProperty(
        "--codeg-message-virtual-overflow-correction"
      )
      content.style.removeProperty("--codeg-conversation-bottom-dock-overlap")
      cancelScheduled()
    }
  }, [cancelScheduled, contentRef, scheduleCorrection, scopeKey, scrollRef])

  useEffect(() => {
    scheduleCorrection()
  }, [layoutSignal, scheduleCorrection])

  return null
})
