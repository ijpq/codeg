"use client"

import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from "react"
import { createPortal } from "react-dom"

import { cn } from "@/lib/utils"

import { ReferenceIcon } from "../badges/reference-badge"
import type { ReferenceAttrs } from "../types"
import type { MentionRenderState } from "./mention-suggestion"
import type {
  ReferenceSearch,
  SuggestionGroup,
  SuggestionPopupHandle,
} from "./types"

const FETCH_DEBOUNCE_MS = 150

export interface SuggestionPopupProps {
  /** Live trigger state (query/range/caret rect). */
  state: MentionRenderState
  /** Resolves the query into grouped suggestions. Must be referentially stable. */
  search: ReferenceSearch
  /** Insert the chosen reference, replacing the trigger range. */
  onSelect: (
    reference: ReferenceAttrs,
    range: { from: number; to: number }
  ) => void
  /** Dismiss the panel without inserting. */
  onClose: () => void
  emptyLabel?: string
  loadingLabel?: string
}

interface FlatRow {
  item: SuggestionGroup["items"][number]
  groupIndex: number
}

/**
 * The unified `@` panel: grouped, keyboard-navigable suggestions positioned at
 * the caret. Keys are forwarded from the suggestion plugin via the imperative
 * handle (the editor keeps DOM focus), so selection is tracked manually rather
 * than relying on focus-based libraries.
 */
export const SuggestionPopup = forwardRef<
  SuggestionPopupHandle,
  SuggestionPopupProps
>(function SuggestionPopup(
  {
    state,
    search,
    onSelect,
    onClose,
    emptyLabel = "No matches",
    loadingLabel = "Searching…",
  },
  ref
) {
  // Results are tagged with the query they answer. While that tag doesn't match
  // the live query (initial mount, or mid-debounce after the query changed) the
  // panel is "stale": it shows loading and nothing is selectable, so Enter can
  // never insert a row from a previous query.
  const [result, setResult] = useState<{
    // null until the first fetch resolves, so results read as "stale"
    // (and the panel shows loading) before any search has answered.
    query: string | null
    groups: SuggestionGroup[]
  }>({ query: null, groups: [] })
  const [selectedIndex, setSelectedIndex] = useState(0)
  const listRef = useRef<HTMLDivElement>(null)
  const stale = result.query !== state.query

  // Debounced, abortable fetch on every query change. All state updates run
  // inside the (async) timer callback, never synchronously in the effect body.
  useEffect(() => {
    const abort = new AbortController()
    let active = true
    const timer = setTimeout(() => {
      Promise.resolve(search(state.query, abort.signal))
        .then((groups) => {
          if (!active || abort.signal.aborted) return
          setResult({ query: state.query, groups })
          setSelectedIndex(0)
        })
        .catch(() => {
          if (!active || abort.signal.aborted) return
          setResult({ query: state.query, groups: [] })
          setSelectedIndex(0)
        })
    }, FETCH_DEBOUNCE_MS)
    return () => {
      active = false
      abort.abort()
      clearTimeout(timer)
    }
  }, [state.query, search])

  // Only fresh results are selectable; selection resets to 0 on each fetch.
  const flat = useMemo<FlatRow[]>(
    () =>
      stale
        ? []
        : result.groups.flatMap((group, groupIndex) =>
            group.items.map((item) => ({ item, groupIndex }))
          ),
    [stale, result.groups]
  )

  // Scroll the active row into view.
  useEffect(() => {
    listRef.current
      ?.querySelector('[data-active="true"]')
      ?.scrollIntoView({ block: "nearest" })
  }, [selectedIndex])

  useImperativeHandle(
    ref,
    (): SuggestionPopupHandle => ({
      onKeyDown: (event) => {
        switch (event.key) {
          case "ArrowDown":
            if (flat.length > 0) {
              setSelectedIndex((index) => (index + 1) % flat.length)
            }
            return true
          case "ArrowUp":
            if (flat.length > 0) {
              setSelectedIndex(
                (index) => (index - 1 + flat.length) % flat.length
              )
            }
            return true
          case "Enter":
          case "Tab": {
            const chosen = flat[selectedIndex]
            if (chosen) onSelect(chosen.item.reference, state.range)
            // No fresh row (still loading, or no matches): consume the key
            // without inserting or submitting. Escape dismisses the panel.
            return true
          }
          case "Escape":
            onClose()
            return true
          default:
            return false
        }
      },
    }),
    [flat, selectedIndex, onSelect, onClose, state.range]
  )

  const rect = state.clientRect
  let rowIndex = -1

  return createPortal(
    <div
      style={{
        position: "fixed",
        left: rect?.left ?? 0,
        top: rect?.top ?? 0,
        zIndex: 50,
      }}
    >
      <div
        ref={listRef}
        data-testid="mention-popup"
        className="absolute bottom-full left-0 mb-1 max-h-72 w-80 overflow-y-auto rounded-xl border border-border bg-popover p-1 text-popover-foreground shadow-lg"
      >
        {stale ? (
          <div className="px-2 py-3 text-sm text-muted-foreground">
            {loadingLabel}
          </div>
        ) : flat.length === 0 ? (
          <div className="px-2 py-3 text-sm text-muted-foreground">
            {emptyLabel}
          </div>
        ) : (
          result.groups.map((group) =>
            group.items.length === 0 ? null : (
              <div key={group.kind} className="py-0.5">
                <div className="px-2 py-1 text-xs font-medium text-muted-foreground">
                  {group.label}
                </div>
                {group.items.map((item) => {
                  rowIndex += 1
                  const active = rowIndex === selectedIndex
                  const index = rowIndex
                  return (
                    <button
                      key={`${group.kind}:${item.reference.id}`}
                      type="button"
                      data-active={active}
                      className={cn(
                        "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm",
                        active
                          ? "bg-accent text-accent-foreground"
                          : "hover:bg-accent/50"
                      )}
                      onMouseDown={(event) => {
                        // Keep editor focus; insert on click.
                        event.preventDefault()
                        onSelect(item.reference, state.range)
                      }}
                      onMouseEnter={() => setSelectedIndex(index)}
                    >
                      <ReferenceIcon data={item.reference} />
                      <span className="flex-1 truncate">
                        {item.reference.label || item.reference.id}
                      </span>
                      {item.detail && (
                        <span className="max-w-[10rem] truncate text-xs text-muted-foreground">
                          {item.detail}
                        </span>
                      )}
                    </button>
                  )
                })}
              </div>
            )
          )
        )}
      </div>
    </div>,
    document.body
  )
})
