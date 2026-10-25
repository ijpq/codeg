import assert from "node:assert/strict"
import { EventEmitter } from "node:events"
import { test } from "node:test"
import { retainedHistory, scopedPageErrors } from "./release-history-check.mjs"

const reply = (id, text) => ({
  id,
  is_user: false,
  blocks: [{ type: "text", text }],
})

test("rollback retains the old answer after more than one native tail page", async () => {
  const turns = [reply("old", "RELEASE_FINAL_LINE")]
  for (let i = 0; i < 47; i++) turns.push(reply(`goal-${i}`, "NATIVE_GOAL"))
  const reads = []
  const result = await retainedHistory(async (options) => {
    reads.push(options)
    const end = options.beforeCursor
      ? Number(options.beforeCursor)
      : turns.length
    const start = Math.max(0, end - options.userTurnLimit)
    return {
      turns: turns.slice(start, end),
      history_page: { has_more: start > 0, next_cursor: String(start) },
    }
  }, "RELEASE_FINAL_LINE")
  assert.deepEqual(result, { pages: 2, turnId: "old" })
  assert.equal(reads.length, 2)
  assert.equal(reads[0].beforeCursor, undefined)
  assert.equal(reads[1].beforeCursor, "23")
  assert.ok(reads.every((options) => options.userTurnLimit === 25))
})

test("retained history does not accept a missing answer or user-only marker", async () => {
  await assert.rejects(
    retainedHistory(
      async () => ({
        turns: [{ ...reply("user", "marker"), is_user: true }],
        history_page: { has_more: false },
      }),
      "marker"
    ),
    /Pre-upgrade content missing/
  )
})

test("repeated opaque cursors fail instead of looping", async () => {
  let reads = 0
  await assert.rejects(
    retainedHistory(async () => {
      reads++
      return {
        turns: [],
        history_page: { has_more: true, next_cursor: "same" },
      }
    }, "marker"),
    /cursor did not advance/
  )
  assert.equal(reads, 2)
})

test("a long history is never loaded without a page bound", async () => {
  let reads = 0
  await assert.rejects(
    retainedHistory(
      async () => ({
        turns: [],
        history_page: { has_more: true, next_cursor: `next-${++reads}` },
      }),
      "marker"
    ),
    /within 4 bounded pages/
  )
  assert.equal(reads, 4)
})

test("real-page errors remain visible but later harness navigation cannot mutate evidence", () => {
  const page = new EventEmitter()
  const active = scopedPageErrors(page)
  page.emit("pageerror", new Error("real app failure"))
  assert.deepEqual(active.errors, ["Error: real app failure"])
  const saved = active.stop()
  page.emit("pageerror", new Error("later isolated fixture"))
  assert.deepEqual(saved, ["Error: real app failure"])
  assert.equal(page.listenerCount("pageerror"), 0)
})
