import { beforeEach, describe, expect, it } from "vitest"

import { loadSortMode, saveSortMode } from "./sidebar-view-mode-storage"

const LEGACY_SORT_MODE_KEY = "workspace:sidebar-sort-mode"
const SORT_MODE_KEY = "workspace:sidebar-sort-mode:v2"

beforeEach(() => {
  localStorage.clear()
})

describe("sidebar sort-mode storage", () => {
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
