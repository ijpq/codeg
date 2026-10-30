import assert from "node:assert/strict"
import { test } from "node:test"
import { resizeAndDismissSidebar } from "./business-agent-page.mjs"

// Model the responsive render boundary: resizing schedules a Drawer mount,
// while Escape only dismisses an already mounted Drawer. Locator names follow
// the actual chrome button's accessible tooltip, including its shortcut.
function sidebarPage(buttonName, dismisses = true) {
  const actions = []
  let scheduled = false
  let visible = false
  const sidebar = {
    isVisible: async () => visible,
    waitFor: async ({ state }) => {
      actions.push(state)
      if (scheduled) {
        visible = true
        scheduled = false
      }
      assert.equal(visible, state === "visible", "Drawer visibility")
    },
  }
  const page = {
    getByRole: (role, { name }) => {
      if (role === "dialog") {
        assert.equal(name, "Sidebar")
        return sidebar
      }
      assert.equal(role, "button")
      const found =
        name instanceof RegExp ? name.test(buttonName) : name === buttonName
      const button = {
        filter: () => button,
        count: async () => Number(found),
      }
      return button
    },
    setViewportSize: async () => {
      actions.push("resize")
      scheduled = buttonName.startsWith("Hide Sidebar")
    },
    keyboard: {
      press: async (key) => {
        assert.equal(key, "Escape")
        actions.push("escape")
        if (dismisses) visible = false
      },
    },
  }
  return { page, actions }
}

for (const name of [
  "Hide Sidebar (Ctrl+B)",
  "Hide Sidebar (Alt+B)",
  "Hide Sidebar",
]) {
  test(`waits for the open sidebar's responsive mount: ${name}`, async () => {
    const { page, actions } = sidebarPage(name)
    const trace = []
    await resizeAndDismissSidebar(page, { width: 390, height: 844 }, trace)
    assert.deepEqual(actions, ["resize", "visible", "escape", "hidden"])
    assert.deepEqual(trace, [
      { phase: "before-resize", sidebarWasOpen: true },
      { phase: "before-escape", sidebarVisible: true },
      { phase: "dismissed" },
    ])
  })
}

test("does not wait for a Drawer mount when the desktop sidebar is closed", async () => {
  const { page, actions } = sidebarPage("Show Sidebar (Ctrl+B)")
  await resizeAndDismissSidebar(page, { width: 390, height: 844 })
  assert.deepEqual(actions, ["resize", "escape", "hidden"])
})

test("an actual dismissal failure still blocks the page gate and preserves its phase", async () => {
  const { page } = sidebarPage("Hide Sidebar", false)
  const trace = []
  await assert.rejects(
    resizeAndDismissSidebar(page, { width: 390, height: 844 }, trace),
    /Drawer visibility/
  )
  assert.equal(trace.at(-1).phase, "before-escape")
  assert.equal(trace.at(-1).sidebarVisible, true)
})
