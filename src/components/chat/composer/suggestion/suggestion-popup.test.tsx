import { act, render, screen } from "@testing-library/react"
import { createRef } from "react"
import { describe, expect, it, vi } from "vitest"

import { SuggestionPopup } from "./suggestion-popup"
import type {
  ReferenceSearch,
  SuggestionGroup,
  SuggestionPopupHandle,
} from "./types"

// Distinct, non-colliding text: a row's label must differ from its detail and
// from the agent icon's <title> ("Codex") so findByText is unambiguous.
const fileRef = {
  refType: "file" as const,
  id: "alpha.md",
  label: "alpha.md",
  uri: "file:///docs/alpha.md",
  meta: null,
}
const agentRef = {
  refType: "agent" as const,
  id: "codex",
  label: "Codex Helper",
  uri: null,
  meta: { agentType: "codex" as const },
}

const groups: SuggestionGroup[] = [
  {
    kind: "file",
    label: "Files",
    items: [{ reference: fileRef, detail: "docs/alpha.md" }],
  },
  { kind: "agent", label: "Agents", items: [{ reference: agentRef }] },
]

const search: ReferenceSearch = () => groups
const emptySearch: ReferenceSearch = () => []

const state = { query: "a", range: { from: 1, to: 3 }, clientRect: null }

function mountPopup(
  overrides: Partial<Parameters<typeof SuggestionPopup>[0]> = {}
) {
  const ref = createRef<SuggestionPopupHandle>()
  const onSelect = vi.fn()
  const onClose = vi.fn()
  render(
    <SuggestionPopup
      ref={ref}
      state={state}
      search={search}
      onSelect={onSelect}
      onClose={onClose}
      {...overrides}
    />
  )
  return { ref, onSelect, onClose }
}

function key(name: string): KeyboardEvent {
  return { key: name } as KeyboardEvent
}

describe("SuggestionPopup", () => {
  it("renders grouped results from the search provider", async () => {
    mountPopup()
    expect(await screen.findByText("alpha.md")).toBeInTheDocument()
    expect(screen.getByText("Files")).toBeInTheDocument()
    expect(screen.getByText("Agents")).toBeInTheDocument()
    expect(screen.getByText("Codex Helper")).toBeInTheDocument()
  })

  it("shows an empty state when there are no matches", async () => {
    mountPopup({ search: emptySearch, emptyLabel: "Nothing" })
    expect(await screen.findByText("Nothing")).toBeInTheDocument()
  })

  it("selects the highlighted row on Enter (default = first)", async () => {
    const { ref, onSelect } = mountPopup()
    await screen.findByText("alpha.md")
    act(() => {
      expect(ref.current?.onKeyDown(key("Enter"))).toBe(true)
    })
    expect(onSelect).toHaveBeenCalledWith(fileRef, state.range)
  })

  it("moves the selection with ArrowDown before selecting", async () => {
    const { ref, onSelect } = mountPopup()
    await screen.findByText("Codex Helper")
    act(() => ref.current?.onKeyDown(key("ArrowDown")))
    act(() => ref.current?.onKeyDown(key("Enter")))
    expect(onSelect).toHaveBeenCalledWith(agentRef, state.range)
  })

  it("wraps the selection with ArrowUp from the first row", async () => {
    const { ref, onSelect } = mountPopup()
    await screen.findByText("Codex Helper")
    act(() => ref.current?.onKeyDown(key("ArrowUp")))
    act(() => ref.current?.onKeyDown(key("Enter")))
    expect(onSelect).toHaveBeenCalledWith(agentRef, state.range)
  })

  it("closes on Escape and reports the key as consumed", async () => {
    const { ref, onClose } = mountPopup()
    await screen.findByText("alpha.md")
    let consumed = false
    act(() => {
      consumed = ref.current?.onKeyDown(key("Escape")) ?? false
    })
    expect(consumed).toBe(true)
    expect(onClose).toHaveBeenCalled()
  })

  it("does not consume unrelated keys", async () => {
    const { ref } = mountPopup()
    await screen.findByText("alpha.md")
    expect(ref.current?.onKeyDown(key("x"))).toBe(false)
  })

  it("does not select stale results after the query changes", async () => {
    const ref = createRef<SuggestionPopupHandle>()
    const onSelect = vi.fn()
    const view = (query: string, to: number) => (
      <SuggestionPopup
        ref={ref}
        state={{ query, range: { from: 1, to }, clientRect: null }}
        search={search}
        onSelect={onSelect}
        onClose={vi.fn()}
        loadingLabel="Loading"
      />
    )
    const { rerender } = render(view("a", 2))
    await screen.findByText("alpha.md") // fresh results for "a"

    // Query advances; the shown results now answer the *previous* query.
    rerender(view("ab", 3))
    expect(screen.queryByText("alpha.md")).toBeNull()
    expect(screen.getByText("Loading")).toBeInTheDocument()

    act(() => ref.current?.onKeyDown(key("Enter")))
    expect(onSelect).not.toHaveBeenCalled()
  })

  it("selects on click (mousedown) and prevents default to keep editor focus", async () => {
    const { onSelect } = mountPopup()
    const label = await screen.findByText("alpha.md")
    const button = label.closest("button")
    expect(button).not.toBeNull()
    const event = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
    })
    act(() => {
      button?.dispatchEvent(event)
    })
    expect(onSelect).toHaveBeenCalledWith(fileRef, state.range)
    // preventDefault keeps focus in the editor rather than the popup button.
    expect(event.defaultPrevented).toBe(true)
  })
})
