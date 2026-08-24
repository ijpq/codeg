import { describe, expect, it, vi } from "vitest"

import { publishConversationDockHeight } from "./conversation-shell"

describe("publishConversationDockHeight", () => {
  it("publishes the measured height of the entire dynamic bottom surface", () => {
    const shell = document.createElement("div")
    const dock = document.createElement("div")
    dock.getBoundingClientRect = vi.fn(
      () =>
        ({
          top: 530,
          bottom: 800,
          height: 270,
          left: 0,
          right: 1200,
          width: 1200,
          x: 0,
          y: 530,
          toJSON: () => ({}),
        }) as DOMRect
    )

    expect(publishConversationDockHeight(shell, dock)).toBe(270)
    expect(
      shell.style.getPropertyValue("--codeg-conversation-bottom-dock-height")
    ).toBe("270px")
    expect(shell).toHaveAttribute("data-bottom-dock-height", "270")
  })
})
