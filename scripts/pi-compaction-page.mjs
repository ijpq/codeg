// Final package composer -> CodeG -> packaged ACP/Pi -> deterministic provider.
// Native compaction really runs; its summarization HTTP response is delayed.
import assert from "node:assert/strict"
import { writeFile } from "node:fs/promises"
import { join } from "node:path"
import { randomUUID } from "node:crypto"
import { openBusiness } from "./business-agent-page.mjs"
import { businessComposer, reloadBusiness } from "./business-composer-page.mjs"
const pause = (ms) => new Promise((done) => setTimeout(done, ms))
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
    await editor.fill(text)
    await editor.press("Enter")
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
      for (let round = 0; round < 4; round++) {
        const marker = `PI_COMPACTION_SEED_${randomUUID()}`
        const seq = await submit(
          marker +
            "\n" +
            "Business requirements, reviewed edits and decisions must remain available.\n".repeat(
              1200
            )
        )
        await reply(marker)
        await idle(seq)
      }
      await idle(await submit("/autocompact on"))
      piModel.tokens(42)
      piModel.compaction(35000)
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
        for (let round = 0; round < 2; round++) {
          const marker = `PI_COMPACTION_REVIEW_${randomUUID()}`
          await idle(
            await submit(
              marker +
                "\n" +
                "Retain reviewed business requirements and decisions.\n".repeat(
                  1800
                )
            )
          )
          await reply(marker)
        }
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
      piModel.compaction(1000)
      await idle(await submit("/compact"))
      await until(
        () => cards("completed").count(),
        (n) => n === 2,
        "Explicit retry completes once"
      )
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
    await writeFile(
      join(output, `${name}-compaction-failure.json`),
      JSON.stringify(
        {
          message: String(error),
          snapshot: await snapshot().catch(() => null),
          requests: piModel.evidence(),
        },
        null,
        2
      )
    )
    throw error
  } finally {
    piModel.tokens(42)
    piModel.compaction(50)
  }
}
