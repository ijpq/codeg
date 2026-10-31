// Real packaged frontends and ACP/Pi runtimes; only the model endpoint and
// private-history-shaped input are synthetic. Never reads a user's profile.
import assert from "node:assert/strict"
import { open, mkdir, writeFile } from "node:fs/promises"
import { createReadStream } from "node:fs"
import { createHash, randomUUID } from "node:crypto"
import { join } from "node:path"
import { openBusiness } from "./business-agent-page.mjs"
import { businessComposer, reloadBusiness } from "./business-composer-page.mjs"

const delay = (ms) => new Promise((done) => setTimeout(done, ms))
async function until(read, accepts, label) {
  const deadline = Date.now() + 180_000
  let value
  do {
    value = await read()
    if (accepts(value)) return value
    await delay(200)
  } while (Date.now() < deadline)
  assert.fail(`${label}: ${JSON.stringify(value)}`)
}
async function digest(path) {
  const hash = createHash("sha256")
  for await (const chunk of createReadStream(path)) hash.update(chunk)
  return hash.digest("hex")
}

export async function writeLongHistoryFixture(f, shape) {
  const id = randomUUID()
  const sessions = join(f.env.CODEX_HOME, "sessions", "2026", "10", "29")
  await mkdir(sessions, { recursive: true })
  const path = join(sessions, `rollout-2026-10-29T00-00-00-${id}.jsonl`)
  const file = await open(path, "wx")
  const record = async (kind, payload, time) =>
    file.writeFile(
      JSON.stringify({ timestamp: time, type: kind, payload }) + "\n"
    )
  const timestamp = "2026-10-29T00:00:00Z"
  try {
    await record(
      "session_meta",
      {
        id,
        cwd: f.cwd,
        originator: "codex_cli_rs",
        cli_version: "0.162.1",
        source: "cli",
        model_provider: "fixture",
      },
      timestamp
    )
    for (let index = 0; index < shape.rounds; index++) {
      const time = new Date(
        Date.parse(timestamp) + index * 300_000
      ).toISOString()
      const turnId = randomUUID()
      await record("event_msg", { type: "task_started", turn_id: turnId }, time)
      await record(
        "turn_context",
        {
          turn_id: turnId,
          cwd: f.cwd,
          model: "gpt-6-astra",
          effort: "medium",
          approval_policy: "never",
          sandbox_policy: { type: "read-only" },
        },
        time
      )
      const text =
        `LONG_HISTORY_USER_${index}` +
        (index === 0
          ? " LONG_HISTORY_FIRST: preserve production; keep template B because A failed validation."
          : "") +
        (index === shape.rounds - 1
          ? " LONG_HISTORY_LATEST: continue the existing task; retain the user correction and latest deliverables."
          : "")
      await record(
        "event_msg",
        { type: "user_message", message: text, images: [] },
        time
      )
      await record(
        "response_item",
        {
          type: "message",
          role: "user",
          content: [{ type: "input_text", text }],
        },
        time
      )
      if (index < shape.heavyRounds) {
        const call = `history-call-${index}`
        await record(
          "response_item",
          {
            type: "function_call",
            call_id: call,
            name: "exec_command",
            arguments: "{}",
          },
          time
        )
        await file.writeFile(
          `{"timestamp":${JSON.stringify(time)},"type":"response_item","payload":{"type":"function_call_output","call_id":${JSON.stringify(call)},"output":"`
        )
        let remaining = Math.floor(
          (shape.mib * 1024 * 1024) / shape.heavyRounds
        )
        const chunk = Buffer.alloc(32 * 1024, "x")
        while (remaining > 0) {
          const take = Math.min(remaining, chunk.length)
          await file.writeFile(chunk.subarray(0, take))
          remaining -= take
        }
        await file.writeFile('"}}\n')
      }
      const answer = `LONG_HISTORY_CONCLUSION_${index}`
      await record(
        "event_msg",
        { type: "agent_message", message: answer, phase: "final_answer" },
        time
      )
      await record(
        "response_item",
        {
          type: "message",
          role: "assistant",
          phase: "final_answer",
          content: [{ type: "output_text", text: answer }],
        },
        time
      )
      await record(
        "event_msg",
        {
          type: "task_complete",
          turn_id: turnId,
          duration_ms: 168795 + index,
          last_agent_message: answer,
        },
        new Date(Date.parse(time) + 168795 + index).toISOString()
      )
    }
  } finally {
    await file.close()
  }
  return { id, path, sha256: await digest(path) }
}

export async function longHistoryHandoffPage({
  page,
  api,
  f,
  name,
  piModel,
  report,
  output,
}) {
  if (!piModel) return
  for (const shape of [
    { name: "first-180mib", rounds: 25, heavyRounds: 25, mib: 180.22 },
    { name: "page13-64mib", rounds: 325, heavyRounds: 25, mib: 64.66 },
    { name: "single-65mib", rounds: 1, heavyRounds: 1, mib: 65 },
  ]) {
    const source = await writeLongHistoryFixture(f, shape)
    const evidence = { shape, source, verified: false, pages: [] }
    try {
      await api("scan_importable_sessions")
      const imported = await api("import_selected_sessions", {
        selections: [{ agentType: "codex", externalId: source.id }],
      })
      assert.equal(imported.failed, 0)
      const business = (await api("list_all_conversations")).find(
        (row) => row.external_id === source.id
      )
      assert.ok(business)
      const cid = business.id,
        folderId = business.folder_id
      evidence.conversationId = cid
      await openBusiness(page, api, cid, folderId, "codex")
      let cursor
      let expectedEnd
      const seen = new Set(),
        users = [],
        conclusions = []
      for (let count = 0; count < shape.rounds + 10; count++) {
        const detail = await until(
          () =>
            api("get_folder_conversation", {
              conversationId: cid,
              beforeCursor: cursor,
              userTurnLimit: 25,
            }),
          (d) => d.history_page?.page_complete === true,
          "Native history page eventually covers every selected complete record"
        )
        const h = detail.history_page
        evidence.pages.push(h)
        if (expectedEnd != null) assert.equal(h.read_end_offset, expectedEnd)
        assert.ok(h.read_start_offset < h.read_end_offset)
        if (h.read_mode !== "streamed_record")
          assert.ok(h.read_end_offset - h.read_start_offset <= 64 * 1024 * 1024)
        users.unshift(
          ...detail.turns
            .filter((t) => t.role === "user")
            .map(
              (t) =>
                JSON.stringify(t.blocks).match(/LONG_HISTORY_USER_\d+/)?.[0]
            )
            .filter(Boolean)
        )
        conclusions.unshift(
          ...detail.turns.flatMap((t) =>
            t.blocks
              .filter((b) => b.type === "text")
              .flatMap(
                (b) => b.text.match(/LONG_HISTORY_CONCLUSION_\d+/g) ?? []
              )
          )
        )
        if (!h.has_more) {
          assert.equal(h.read_start_offset, 0)
          break
        }
        assert.ok(h.next_cursor && !seen.has(h.next_cursor))
        seen.add(h.next_cursor)
        expectedEnd = h.read_start_offset
        cursor = h.next_cursor
      }
      assert.deepEqual(
        users,
        Array.from({ length: shape.rounds }, (_, i) => `LONG_HISTORY_USER_${i}`)
      )
      assert.deepEqual(
        conclusions,
        Array.from(
          { length: shape.rounds },
          (_, i) => `LONG_HISTORY_CONCLUSION_${i}`
        )
      )
      if (shape.name === "page13-64mib")
        assert.equal(evidence.pages[12].read_mode, "byte_budget")
      if (shape.name === "single-65mib")
        assert.ok(evidence.pages.some((p) => p.read_mode === "streamed_record"))
      const before = piModel.evidence().length
      await page
        .locator("[data-business-agent-toolbar]:visible select")
        .first()
        .selectOption("pi")
      const state = await until(
        () => api("get_business_conversation_state", { conversationId: cid }),
        (s) => {
          if (s?.switchStatus === "failed") throw Error(s.switchError)
          return s?.activeAgent === "pi" && s.switchStatus === "succeeded"
        },
        "Actual frontend Codex to Pi handoff"
      )
      assert.equal(state.conversationId, cid)
      assert.equal(
        state.sessions.find((s) => s.agentType === "codex").externalId,
        source.id
      )
      assert.notEqual(
        state.sessions.find((s) => s.agentType === "pi").externalId,
        source.id
      )
      await until(
        () =>
          page
            .locator("[data-business-agent-toolbar]:visible select")
            .first()
            .inputValue(),
        (v) => v === "pi",
        "Frontend observes successful switch"
      )
      const sent = []
      for (let round = 0; round < 2; round++) {
        const input =
          round === 0
            ? await businessComposer(page, { conversationId: cid, agent: "pi" })
            : await reloadBusiness(page, api, {
                conversationId: cid,
                folderId,
                agent: "pi",
              })
        const text = `LONG_HISTORY_SEND_${randomUUID()}`
        sent.push(text)
        await input.fill(text)
        await input.press("Enter")
        await until(
          () =>
            api("get_folder_conversation", {
              conversationId: cid,
              userTurnLimit: 25,
            }),
          (d) => {
            const i = d.turns.findIndex(
              (t) =>
                t.role === "user" && JSON.stringify(t.blocks).includes(text)
            )
            return (
              i >= 0 &&
              d.turns
                .slice(i + 1)
                .some(
                  (t) =>
                    t.role === "assistant" &&
                    JSON.stringify(t.blocks).includes(
                      text.replace("_SEND_", "_REPLY_")
                    )
                )
            )
          },
          "Frontend message after long-history switch actually executes and replies"
        )
        await page
          .locator(".chat-scroll-port:visible")
          .getByText(text.replace("_SEND_", "_REPLY_"), { exact: false })
          .last()
          .waitFor({ timeout: 90_000 })
      }
      const requests = piModel.evidence().slice(before)
      for (const marker of sent)
        assert.equal(
          requests.filter((r) => r.longHistorySend === marker).length,
          1,
          "No duplicate model execution"
        )
      assert.ok(
        requests.some(
          (r) =>
            r.longHistoryFirst &&
            r.longHistoryLatest &&
            r.longHistoryReason &&
            r.longHistoryBoundary
        ),
        "Target receives both early requirements/reasons and recent context; a UI history alone is insufficient"
      )
      assert.equal(
        await digest(source.path),
        source.sha256,
        "Original source remains byte-for-byte intact"
      )
      evidence.verified = true
      evidence.targetRequests = requests
      evidence.state = state
      await page.screenshot({
        path: join(output, `${name}-${shape.name}-handoff.png`),
      })
      report.checks.push({
        name: `${name}-long-history-${shape.name}`,
        passed: true,
        pages: evidence.pages.length,
        actualFrontend: true,
        packagedRuntime: true,
        syntheticModel: true,
      })
    } finally {
      await writeFile(
        join(output, `${name}-${shape.name}-handoff.json`),
        JSON.stringify(evidence, null, 2)
      )
    }
  }
}
