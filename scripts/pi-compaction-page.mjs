// Final package composer -> CodeG -> packaged ACP/Pi -> deterministic provider.
// Native compaction really runs; its summarization HTTP response is delayed.
import assert from "node:assert/strict"
import { writeFile } from "node:fs/promises"
import { join } from "node:path"
import { randomUUID } from "node:crypto"
import { openBusiness } from "./business-agent-page.mjs"
import { businessComposer, reloadBusiness } from "./business-composer-page.mjs"
const pause = (ms) => new Promise((done) => setTimeout(done, ms))
export async function submitCompactionComposer(page, editor, text) {
  await editor.fill(text)
  if (text.startsWith("/")) {
    // Enter accepts the slash-menu suggestion; it is not a send receipt.
    // Click the real composer affordance once, without a second Enter/retry.
    await page
      .getByRole("button", { name: /^(Send|发送|傳送)$/ })
      .filter({ visible: true })
      .last()
      .click()
  } else {
    await editor.press("Enter")
  }
}
async function until(read, accepts, label) {
  let value
  const deadline = Date.now() + 90_000
  do {
    value = await read()
    if (accepts(value)) return value
    await pause(100)
  } while (Date.now() < deadline)
  assert.fail(`${label}: ${JSON.stringify(value)}`)
}
// A long native transcript virtualizes older cards. Verify persisted identities
// independently of how many historical cards happen to be mounted in the DOM.
export function compactionHistoryRecords(history) {
  return (history?.turns ?? []).flatMap((turn) =>
    (turn.blocks ?? []).flatMap((block) => {
      const compaction = block.meta?.contextCompaction
      if (block.type !== "tool_use" || !compaction) return []
      return [
        {
          eventId: compaction.eventId ?? null,
          status: compaction.status ?? null,
          turnId: turn.id ?? null,
        },
      ]
    })
  )
}
export function assertCompletedCompactionAdvance(before, after) {
  const completed = (records) => records.filter((r) => r.status === "completed")
  const previous = completed(before)
  const current = completed(after)
  assert.ok(
    current.every(
      (record) => typeof record.eventId === "string" && record.eventId
    ),
    "Completed native compactions have stable event identities"
  )
  const beforeIds = new Set(previous.map((record) => record.eventId))
  const afterIds = new Set(current.map((record) => record.eventId))
  assert.ok(
    [...beforeIds].every((id) => afterIds.has(id)),
    "The bounded test history retains earlier native compactions"
  )
  const added = current.filter((record) => !beforeIds.has(record.eventId))
  assert.equal(
    added.length,
    1,
    "Exactly one new native compaction is persisted"
  )
  assert.equal(afterIds.size, beforeIds.size + 1)
  return added[0]
}
async function completedCompactionAdvance(history, before) {
  const previous = new Set(
    before.filter((r) => r.status === "completed").map((r) => r.eventId)
  )
  const after = await until(
    async () => compactionHistoryRecords(await history()),
    (records) =>
      records.some(
        (r) => r.status === "completed" && r.eventId && !previous.has(r.eventId)
      ),
    "A new completed native compaction is persisted"
  )
  return {
    record: assertCompletedCompactionAdvance(before, after),
    records: after,
  }
}
export async function piCompactionPage({
  page,
  api,
  folderId,
  piModel,
  report,
  name,
  output,
  saved = null,
}) {
  if (!piModel) return null
  const cid =
    saved?.cid ??
    (await api("create_conversation", {
      folderId,
      agentType: "pi",
      title: "Native long-history compaction",
    }))
  const target = { conversationId: cid, folderId, agent: "pi" }
  await openBusiness(page, api, cid, folderId, "pi")
  const history = () =>
    api("get_folder_conversation", { conversationId: cid, userTurnLimit: 25 })
  if (!saved) {
    // An empty business has no native session to find yet. Its first real
    // composer submission establishes that binding; never prewarm it via ACP.
    const marker = `PI_COMPACTION_BOOTSTRAP_${randomUUID()}`
    const editor = await businessComposer(page, target)
    await editor.fill(marker)
    await editor.press("Enter")
    await until(
      history,
      (h) => {
        const index = h.turns.findIndex(
          (t) => t.role === "user" && JSON.stringify(t.blocks).includes(marker)
        )
        return (
          index >= 0 &&
          h.turns
            .slice(index + 1)
            .some(
              (t) =>
                t.role === "assistant" &&
                JSON.stringify(t.blocks).includes("BUSINESS_PI_COMPLETED")
            )
        )
      },
      "First composer message establishes a real Pi business session"
    )
  }
  const live = await until(
    () =>
      api("acp_find_connection_for_conversation", {
        conversationId: cid,
        agentType: "pi",
      }),
    (v) => !!v?.connection_id,
    "Business Pi is attached"
  )
  const snapshot = () =>
    api("acp_get_session_snapshot", { connectionId: live.connection_id })
  await until(
    snapshot,
    (s) => s.selectors_ready && s.status === "connected",
    "Packaged Pi ready"
  )
  const model = (await snapshot()).config_options.find((c) => c.id === "model")
  const options = [
    ...model.kind.options,
    ...model.kind.groups.flatMap((g) => g.options),
  ]
  const gpt = options.find(
    (o) => o.value?.includes("fixture/") && o.value?.includes("gpt-5.4")
  )
  assert.ok(gpt, "Use this profile's isolated GPT provider")
  await api("acp_set_config_option", {
    connectionId: live.connection_id,
    configId: "model",
    valueId: gpt.value,
  })
  const submit = async (text) => {
    const before = await snapshot()
    const editor = await businessComposer(page, target)
    await submitCompactionComposer(page, editor, text)
    return before.event_seq
  }
  const idle = (seq) =>
    until(
      snapshot,
      (s) => s.event_seq > seq && s.status === "connected" && !s.turn_in_flight,
      "Composer task actually ended"
    )
  const reply = async (marker) => {
    await until(
      history,
      (h) => {
        const index = h.turns.findIndex(
          (t) => t.role === "user" && JSON.stringify(t.blocks).includes(marker)
        )
        return (
          index >= 0 &&
          h.turns
            .slice(index + 1)
            .some(
              (t) =>
                t.role === "assistant" &&
                JSON.stringify(t.blocks).includes("BUSINESS_PI_COMPLETED")
            )
        )
      },
      "Native reply persisted for the actual composer message"
    )
    const h = await history()
    assert.equal(
      h.turns.filter(
        (t) => t.role === "user" && JSON.stringify(t.blocks).includes(marker)
      ).length,
      1
    )
    assert.equal(
      piModel
        .evidence()
        .filter((r) => !r.compaction && r.compactionMarker === marker).length,
      1,
      "The native provider received this composer message exactly once"
    )
  }
  const cards = (status) =>
    page.locator(
      `[data-testid="context-compaction"][data-compaction-status="${status}"]:visible`
    )
  const checks = []
  const completedEvents = []
  try {
    if (saved) {
      assert.equal(
        await cards("in_progress").count(),
        0,
        "Restart does not resurrect a running compaction"
      )
      const marker = `PI_COMPACTION_RESTART_${randomUUID()}`
      const seq = await submit(marker)
      await reply(marker)
      await idle(seq)
      checks.push("restart-native-history-and-next-composer-reply")
    } else {
      await idle(await submit("/autocompact off"))
      piModel.tokens(120000)
      // Accumulate actual native history via four real composer turns and long
      // provider replies. Do not make giant multiline editor injection a
      // precondition for testing preflight compaction.
      piModel.historyReply(true)
      for (let round = 0; round < 4; round++) {
        const marker = `PI_COMPACTION_SEED_${randomUUID()}`
        const seq = await submit(marker)
        await reply(marker)
        await idle(seq)
      }
      piModel.historyReply(false)
      await idle(await submit("/autocompact on"))
      piModel.tokens(42)
      piModel.compaction(35000)
      const beforeCompaction = compactionHistoryRecords(await history())
      const started = Date.now(),
        marker = `PI_COMPACTION_CONTINUE_${randomUUID()}`
      const seq = await submit(marker)
      await cards("in_progress").first().waitFor({ timeout: 15000 })
      assert.match(
        await cards("in_progress").first().innerText(),
        /Compacting context|正在压缩上下文/
      )
      // The active writer must survive leaving the business, returning and a cold page attach.
      const identity = await page
        .locator("[data-session-readiness]:visible")
        .first()
        .evaluate((n) => JSON.parse(n.getAttribute("data-session-readiness")))
      await page
        .locator(".tab-strip-tail")
        .getByRole("button", { name: /^(New Conversation|新建会话|新增會話)$/ })
        .first()
        .click()
      await page
        .locator(`[data-tab-id="${identity.contextKey}"] [role="tab"]`)
        .click()
      await reloadBusiness(page, api, target)
      await cards("in_progress").first().waitFor({ timeout: 15000 })
      if (name === "server-web") {
        const second = await page.context().newPage()
        try {
          await second.goto(page.url())
          await second
            .locator(
              '[data-testid="context-compaction"][data-compaction-status="in_progress"]:visible'
            )
            .first()
            .waitFor({ timeout: 15000 })
          assert.equal(
            await second
              .locator(
                '[data-testid="context-compaction"][data-compaction-status="in_progress"]:visible'
              )
              .count(),
            1
          )
          checks.push("second-client-attaches-to-the-same-running-compaction")
        } finally {
          await second.close()
        }
      }
      await until(
        () => Promise.resolve(Date.now() - started),
        (v) => v > 31000,
        "Native compaction exceeds the old 30-second deadline"
      )
      const during = await snapshot()
      assert.ok(
        during.turn_in_flight || during.status === "prompting",
        "ACP has not falsely ended the task"
      )
      assert.equal(await cards("in_progress").count(), 1)
      assert.match(await cards("in_progress").first().innerText(), /\d+s/)
      await page.screenshot({
        path: join(output, `${name}-compaction-over-30s.png`),
      })
      await reply(marker)
      await idle(seq)
      const initialCompletion = await completedCompactionAdvance(
        history,
        beforeCompaction
      )
      completedEvents.push(initialCompletion.record)
      assert.ok(
        piModel.evidence().some((r) => r.compaction && r.inputBytes > 120000),
        "Native summarization received the actual accumulated long history"
      )
      await until(
        () => cards("completed").count(),
        (n) => n === 1,
        "Live/native history converge to one completed record"
      )
      assert.equal(await cards("in_progress").count(), 0)
      checks.push(
        "long-native-preflight-over-30s-reply-once",
        "switch-away-return-reload-active-phase",
        "single-completion-record"
      )
      const next = `PI_COMPACTION_NEXT_${randomUUID()}`
      await idle(await submit(next))
      await reply(next)
      await idle(await submit("/autocompact off"))
      const addHistory = async () => {
        piModel.historyReply(true)
        for (let round = 0; round < 2; round++) {
          const marker = `PI_COMPACTION_REVIEW_${randomUUID()}`
          await idle(await submit(marker))
          await reply(marker)
        }
        piModel.historyReply(false)
      }
      await addHistory()
      piModel.compaction(500, true)
      await idle(await submit("/compact"))
      await cards("failed").last().waitFor({ timeout: 15000 })
      assert.match(
        await cards("failed").last().locator("[title]").getAttribute("title"),
        /403|denied/
      )
      checks.push("native-compaction-failure-retains-provider-reason")
      const beforeRetry = compactionHistoryRecords(await history())
      assert.deepEqual(
        beforeRetry
          .filter((r) => r.status === "completed")
          .map((r) => r.eventId),
        initialCompletion.records
          .filter((r) => r.status === "completed")
          .map((r) => r.eventId),
        "Failed native compaction does not fabricate a successful record"
      )
      piModel.compaction(1000)
      await idle(await submit("/compact"))
      const retryCompletion = await completedCompactionAdvance(
        history,
        beforeRetry
      )
      completedEvents.push(retryCompletion.record)
      await cards("completed").last().waitFor({ timeout: 15000 })
      assert.equal(await cards("in_progress").count(), 0)
      checks.push("explicit-retry-persists-one-distinct-native-completion")
      await addHistory()
      piModel.compaction(35000)
      const cancelSeq = await submit("/compact")
      await cards("in_progress").first().waitFor({ timeout: 15000 })
      await page
        .getByRole("button", { name: /^(Stop|Cancel|停止|取消)$/ })
        .filter({ visible: true })
        .last()
        .click()
      await idle(cancelSeq)
      assert.equal(await cards("in_progress").count(), 0)
      await cards("cancelled").last().waitFor({ timeout: 15000 })
      const retry = `PI_COMPACTION_CANCEL_RETRY_${randomUUID()}`
      await idle(await submit(retry))
      await reply(retry)
      await reloadBusiness(page, api, target)
      assert.equal(await cards("in_progress").count(), 0)
      await idle(await submit("/autocompact on"))
      checks.push(
        "stop-native-compaction-no-ghost-execution",
        "next-composer-message-after-failure-and-stop"
      )
    }
    await page.screenshot({
      path: join(
        output,
        `${name}-compaction-settled${saved ? "-restart" : ""}.png`
      ),
    })
    report.checks.push({
      name: `${name}-native-compaction-composer${saved ? "-restart" : ""}`,
      checks,
      conversationId: cid,
      provider: "isolated deterministic HTTP; native packaged compaction",
    })
    await writeFile(
      join(output, `${name}-pi-compaction${saved ? "-restart" : ""}.json`),
      JSON.stringify(
        {
          checks,
          conversationId: cid,
          completedEvents,
          compactionRecords: compactionHistoryRecords(await history()),
          requests: piModel
            .evidence()
            .filter((r) => r.compaction || r.compactionMarker),
          snapshot: await snapshot(),
        },
        null,
        2
      )
    )
    return { cid }
  } catch (error) {
    const editor = await page
      .locator('[contenteditable="true"]')
      .filter({ visible: true })
      .first()
      .evaluate(
        (element) => ({
          textLength: element.textContent?.length ?? 0,
          editable: element.getAttribute("contenteditable"),
          disabled: element.getAttribute("aria-disabled"),
          ready: [...document.querySelectorAll("[data-session-readiness]")]
            .filter((node) => node.getClientRects().length > 0)
            .map((node) => node.getAttribute("data-session-readiness")),
        }),
        undefined,
        { timeout: 5000 }
      )
      .catch(() => null)
    await page
      .screenshot({
        path: join(output, `${name}-compaction-failure.png`),
        timeout: 5000,
      })
      .catch(() => null)
    await writeFile(
      join(output, `${name}-compaction-failure.json`),
      JSON.stringify(
        {
          message: String(error),
          editor,
          compactionRecords: compactionHistoryRecords(
            await history().catch(() => null)
          ),
          snapshot: await snapshot().catch(() => null),
          requests: piModel.evidence(),
        },
        null,
        2
      )
    )
    throw error
  } finally {
    piModel.historyReply(false)
    piModel.tokens(42)
    piModel.compaction(50)
  }
}
