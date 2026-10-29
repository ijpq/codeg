import assert from "node:assert/strict"
import { test } from "node:test"
import {
  assertComposerIdentity,
  reloadBusiness,
} from "./business-composer-page.mjs"

const target = { conversationId: 2, folderId: 1, agent: "pi" }
const restoring = {
  contextKey: "conv-1-pi-2",
  conversationId: 2,
  selectedAgent: "pi",
  connectionReady: false,
  promptReady: false,
}
const snapshot = {
  visibleEditors: 1,
  focusedTabs: [restoring.contextKey],
  readiness: [restoring],
}

test("the upgraded business can queue input before the native session is ready", () => {
  assert.equal(assertComposerIdentity(snapshot, target), restoring)
})

test("the failed draft's ready detour must not receive another business's input", () => {
  assert.throws(() =>
    assertComposerIdentity(
      {
        ...snapshot,
        focusedTabs: ["conv-1-pi-3"],
        readiness: [
          {
            ...restoring,
            contextKey: "conv-1-pi-3",
            conversationId: 3,
            connectionReady: true,
            promptReady: true,
          },
        ],
      },
      target
    )
  )
})

test("missing identity, stale panel, another Agent or multiple editors fail before send", () => {
  for (const change of [
    { readiness: [] },
    { focusedTabs: ["conv-1-pi-3"] },
    { readiness: [{ ...restoring, selectedAgent: "codex" }] },
    { visibleEditors: 2 },
  ])
    assert.throws(() =>
      assertComposerIdentity({ ...snapshot, ...change }, target)
    )
})

test("reload waits for durable focus, verifies identity and never warms a connection", async () => {
  const calls = []
  let reloaded = false
  let reads = 0
  const page = {
    reload: async () => {
      reloaded = true
    },
    locator: () => ({
      filter() {
        return this
      },
      first() {
        return this
      },
      waitFor: async () => assert.equal(reloaded, true),
    }),
    evaluate: async () => snapshot,
  }
  await reloadBusiness(
    page,
    async (command) => {
      calls.push(command)
      assert.equal(reloaded, false)
      return {
        items: [
          {
            is_active: true,
            folder_id: 1,
            agent_type: "pi",
            conversation_id: ++reads === 1 ? 3 : 2,
          },
        ],
      }
    },
    target
  )
  assert.deepEqual(calls, ["list_opened_tabs", "list_opened_tabs"])
  assert.equal(reloaded, true)
})
