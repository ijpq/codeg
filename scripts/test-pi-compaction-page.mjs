import assert from "node:assert/strict"
import { test } from "node:test"
import { piCompactionPage } from "./pi-compaction-page.mjs"

// This adapter deliberately creates a binding only after a composer send.
// Looking for a ready native session first reproduced the failed Windows gate.
async function runAdmission(saved) {
  const calls = []
  let marker
  const reached = new Error(
    "native session admitted; stop before long-history scenario"
  )
  const node = {
    first() {
      return this
    },
    filter() {
      return this
    },
    waitFor: async () => {},
    click: async () => {},
    getAttribute: async () => "true",
    fill: async (text) => {
      marker = text
      calls.push("fill")
    },
    press: async (key) => {
      assert.equal(key, "Enter")
      calls.push("composer-send")
    },
  }
  const page = {
    reload: async () => {},
    locator: () => node,
    evaluate: async () => ({
      visibleEditors: 1,
      focusedTabs: ["conv-1-pi-7"],
      readiness: [
        {
          contextKey: "conv-1-pi-7",
          conversationId: 7,
          selectedAgent: "pi",
          promptReady: false,
        },
      ],
    }),
  }
  const api = async (command) => {
    calls.push(command)
    if (command === "create_conversation") return 7
    if (command === "list_opened_tabs") return { version: 1, items: [] }
    if (command === "save_opened_tabs") return { accepted: true }
    if (command === "get_folder_conversation") {
      assert.ok(
        calls.includes("composer-send"),
        "History is checked after an actual send"
      )
      return {
        turns: [
          { role: "user", blocks: [{ text: marker }] },
          { role: "assistant", blocks: [{ text: "BUSINESS_PI_COMPLETED" }] },
        ],
      }
    }
    if (command === "acp_find_connection_for_conversation") {
      assert.ok(
        saved || calls.includes("composer-send"),
        "Empty businesses cannot bind before their first composer message"
      )
      return { connection_id: "native-connection" }
    }
    if (command === "acp_get_session_snapshot") throw reached
    throw new Error("Unexpected command: " + command)
  }
  await assert.rejects(
    piCompactionPage({
      page,
      api,
      folderId: 1,
      piModel: {},
      report: { checks: [] },
      name: "isolated",
      output: ".",
      saved,
    }),
    (error) => error === reached
  )
  return { calls, marker }
}

test("new compaction business binds through one real composer input before looking up native readiness", async () => {
  const { calls, marker } = await runAdmission(null)
  assert.match(marker, /^PI_COMPACTION_BOOTSTRAP_/)
  assert.equal(calls.filter((c) => c === "composer-send").length, 1)
  assert.ok(
    calls.indexOf("composer-send") <
      calls.indexOf("acp_find_connection_for_conversation")
  )
  assert.equal(calls.includes("acp_connect"), false)
  assert.equal(calls.includes("acp_prompt"), false)
})

test("restored compaction business does not replay the initial message", async () => {
  const { calls } = await runAdmission({ cid: 7 })
  assert.equal(calls.includes("composer-send"), false)
  assert.equal(calls.includes("create_conversation"), false)
})
