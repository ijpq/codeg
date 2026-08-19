import { beforeEach, describe, expect, it } from "vitest"

import {
  DEFAULT_SECTION_ORDER,
  loadSectionCollapsed,
  loadSectionOrder,
  loadShowRecent,
  loadSortMode,
  moveSectionInOrder,
  normalizeSectionOrder,
  saveSectionOrder,
  saveShowRecent,
  saveSortMode,
} from "./sidebar-view-mode-storage"

const LEGACY_SECTION_ORDER_KEY = "workspace:sidebar-section-order"
const SECTION_ORDER_KEY = "workspace:sidebar-section-order:v2"
const SHOW_RECENT_KEY = "workspace:sidebar-show-recent"
const LEGACY_SORT_MODE_KEY = "workspace:sidebar-sort-mode"
const SORT_MODE_KEY = "workspace:sidebar-sort-mode:v2"

describe("normalizeSectionOrder", () => {
  it("passes a complete order through unchanged", () => {
    expect(normalizeSectionOrder(["recent", "chats", "folders"])).toEqual([
      "recent",
      "chats",
      "folders",
    ])
  })

  it("appends missing sections in default order", () => {
    // Forward compatibility in reverse: a store written before "Recent" existed
    // gets it at the bottom rather than losing the section entirely.
    expect(normalizeSectionOrder(["chats", "folders"])).toEqual([
      "chats",
      "folders",
      "recent",
    ])
  })

  it("drops unknown entries and repeats", () => {
    expect(
      normalizeSectionOrder(["recent", "bogus", "recent", 7, null, "chats"])
    ).toEqual(["recent", "chats", "folders"])
  })

  it("migrates the legacy two-way strings, preserving the user's choice", () => {
    expect(normalizeSectionOrder("chats-first")).toEqual([
      "chats",
      "folders",
      "recent",
    ])
    expect(normalizeSectionOrder("folders-first")).toEqual([
      "folders",
      "chats",
      "recent",
    ])
  })

  it("falls back to the default for anything unusable", () => {
    expect(normalizeSectionOrder(undefined)).toEqual(DEFAULT_SECTION_ORDER)
    expect(normalizeSectionOrder("nonsense")).toEqual(DEFAULT_SECTION_ORDER)
    expect(normalizeSectionOrder({ folders: 1 })).toEqual(DEFAULT_SECTION_ORDER)
    expect(normalizeSectionOrder([])).toEqual(DEFAULT_SECTION_ORDER)
  })
})

describe("moveSectionInOrder", () => {
  const order = ["folders", "chats", "recent"] as const

  it("moves a section one slot in either direction", () => {
    expect(moveSectionInOrder(order, "recent", -1)).toEqual([
      "folders",
      "recent",
      "chats",
    ])
    expect(moveSectionInOrder(order, "folders", 1)).toEqual([
      "chats",
      "folders",
      "recent",
    ])
  })

  it("moves across multiple slots", () => {
    expect(moveSectionInOrder(order, "recent", -2)).toEqual([
      "recent",
      "folders",
      "chats",
    ])
  })

  it("returns the SAME reference for a move that would fall off an end", () => {
    // Identity matters: the sidebar skips the state update and the localStorage
    // write when a clamped nudge changes nothing.
    expect(moveSectionInOrder(order, "folders", -1)).toBe(order)
    expect(moveSectionInOrder(order, "recent", 1)).toBe(order)
    expect(moveSectionInOrder(order, "chats", 0)).toBe(order)
  })
})

describe("section-order persistence", () => {
  beforeEach(() => localStorage.clear())

  it("round-trips through localStorage", () => {
    saveSectionOrder(["recent", "folders", "chats"])
    expect(loadSectionOrder()).toEqual(["recent", "folders", "chats"])
  })

  it("defaults to Recent → Folders → Chat with nothing stored", () => {
    expect(loadSectionOrder()).toEqual(["recent", "folders", "chats"])
  })

  it("migrates a legacy order and promotes Recent while preserving the rest", () => {
    // The old format was not JSON, so the loader must survive the parse failure
    // rather than resetting the user's Folders/Chat preference.
    localStorage.setItem(LEGACY_SECTION_ORDER_KEY, "chats-first")
    expect(loadSectionOrder()).toEqual(["recent", "chats", "folders"])
    expect(JSON.parse(localStorage.getItem(SECTION_ORDER_KEY) ?? "")).toEqual([
      "recent",
      "chats",
      "folders",
    ])
  })

  it("falls back to the default for corrupt JSON", () => {
    localStorage.setItem(SECTION_ORDER_KEY, "{oops")
    expect(loadSectionOrder()).toEqual(DEFAULT_SECTION_ORDER)
  })
})

describe("loadShowRecent", () => {
  beforeEach(() => localStorage.clear())

  it("defaults to on", () => {
    expect(loadShowRecent()).toBe(true)
  })

  it("respects an explicitly-stored false", () => {
    saveShowRecent(false)
    expect(localStorage.getItem(SHOW_RECENT_KEY)).toBe("false")
    expect(loadShowRecent()).toBe(false)
    saveShowRecent(true)
    expect(loadShowRecent()).toBe(true)
  })
})

describe("loadSectionCollapsed", () => {
  beforeEach(() => localStorage.clear())

  it("reads the recent section's collapsed flag", () => {
    localStorage.setItem(
      "workspace:sidebar-section-collapsed",
      JSON.stringify({ recent: true, chats: false, bogus: 1 })
    )
    expect(loadSectionCollapsed()).toEqual({ recent: true, chats: false })
  })
})

describe("sidebar sort-mode storage", () => {
  beforeEach(() => localStorage.clear())

  it("defaults new installations to latest activity", () => {
    expect(loadSortMode()).toBe("updated")
  })

  it("migrates the historical created-time default to latest activity", () => {
    localStorage.setItem(LEGACY_SORT_MODE_KEY, "created")

    expect(loadSortMode()).toBe("updated")
  })

  it("persists an explicit choice made after the default migration", () => {
    saveSortMode("created")

    expect(localStorage.getItem(SORT_MODE_KEY)).toBe("created")
    expect(loadSortMode()).toBe("created")
  })

  it("falls back to latest activity for an invalid stored value", () => {
    localStorage.setItem(SORT_MODE_KEY, "unexpected")

    expect(loadSortMode()).toBe("updated")
  })
})
