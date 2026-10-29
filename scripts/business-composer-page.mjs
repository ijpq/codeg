import assert from "node:assert/strict"

export function assertComposerIdentity(snapshot, target) {
  assert.equal(snapshot.visibleEditors, 1, "One business composer is visible")
  assert.equal(snapshot.focusedTabs.length, 1, "One business tab is selected")
  assert.equal(snapshot.readiness.length, 1, "One business identity is visible")
  const identity = snapshot.readiness[0]
  assert.equal(identity.contextKey, snapshot.focusedTabs[0])
  assert.equal(identity.conversationId, target.conversationId)
  assert.equal(identity.selectedAgent, target.agent)
  // Identity is required, readiness is deliberately not: the input must still
  // exercise its durable queue while the real native session is restoring.
  return identity
}

export async function businessComposer(page, target) {
  const inputs = page
    .locator('[contenteditable="true"]')
    .filter({ visible: true })
  await inputs.first().waitFor({ timeout: 90_000 })
  await page.locator("[data-session-readiness]:visible").first().waitFor({
    timeout: 90_000,
  })
  const snapshot = await page.evaluate(() => ({
    visibleEditors: [
      ...document.querySelectorAll('[contenteditable="true"]'),
    ].filter((element) => element.getClientRects().length > 0).length,
    focusedTabs: [
      ...document.querySelectorAll(
        '[data-tab-id] [role="tab"][aria-selected="true"]'
      ),
    ]
      .filter((element) => element.getClientRects().length > 0)
      .map((element) =>
        element.closest("[data-tab-id]").getAttribute("data-tab-id")
      ),
    readiness: [...document.querySelectorAll("[data-session-readiness]")]
      .filter((element) => element.getClientRects().length > 0)
      .map((element) =>
        JSON.parse(element.getAttribute("data-session-readiness"))
      ),
  }))
  assertComposerIdentity(snapshot, target)
  return inputs.first()
}

export async function reloadBusiness(page, api, target) {
  // Focus is saved with the normal 500ms tab debounce. Establish that durable
  // precondition before testing cold restoration; never prewarm an ACP session
  // or reselect the tab after reload, which would conceal a restore defect.
  const deadline = Date.now() + 30_000
  let active
  do {
    const tabs = await api("list_opened_tabs")
    active = tabs.items.find((tab) => tab.is_active)
    if (
      active?.conversation_id === target.conversationId &&
      active.folder_id === target.folderId &&
      active.agent_type === target.agent
    ) {
      await page.reload()
      return businessComposer(page, target)
    }
    await new Promise((done) => setTimeout(done, 100))
  } while (Date.now() < deadline)
  assert.fail(`Business focus was not persisted: ${JSON.stringify(active)}`)
}
