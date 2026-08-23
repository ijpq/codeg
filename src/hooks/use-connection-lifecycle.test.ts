import { describe, expect, it } from "vitest"
import { shouldReleaseSurfaceOnUnmount } from "@/hooks/use-connection-lifecycle"

describe("shouldReleaseSurfaceOnUnmount", () => {
  it("releases a normal tab/component surface", () => {
    expect(shouldReleaseSurfaceOnUnmount({})).toBe(true)
    expect(shouldReleaseSurfaceOnUnmount({ transientUnmount: false })).toBe(
      true
    )
  })

  it("keeps the surface through a transient split/tile reparent", () => {
    expect(shouldReleaseSurfaceOnUnmount({ transientUnmount: true })).toBe(
      false
    )
  })
})
