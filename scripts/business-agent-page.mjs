// Exercises the packaged app and real ACP adapters against deterministic local
// model endpoints. No live model login or reasoning is claimed by this check.
import assert from "node:assert/strict"
import { mkdir, writeFile } from "node:fs/promises"
import { join } from "node:path"
import { randomUUID } from "node:crypto"

const delay = (ms) => new Promise((done) => setTimeout(done, ms))
async function until(read, accepts, label) {
  let value
  for (let i = 0; i < 900; i++) {
    value = await read()
    if (accepts(value)) return value
    await delay(100)
  }
  throw Error(`${label}: ${JSON.stringify(value)}`)
}
async function openBusiness(page, api, cid, folderId, agent) {
  const tabs = await api("list_opened_tabs")
  const saved = await api("save_opened_tabs", {
    expectedVersion: tabs.version,
    origin: "isolated-business-verification",
    items: [
      {
        id: 0,
        folder_id: folderId,
        conversation_id: cid,
        agent_type: agent,
        position: 0,
        is_active: true,
        is_pinned: false,
      },
    ],
  })
  assert.equal(saved.accepted, true)
  await page.reload()
  await page
    .locator("[data-business-agent-control]:visible select")
    .first()
    .waitFor({ timeout: 90_000 })
}

export async function businessAgentPage({
  page,
  api,
  f,
  conversation,
  name,
  goalModel,
  claudeModel,
  report,
  output,
}) {
  if (!goalModel || !claudeModel) return null
  goalModel.business(true)
  const folderId = conversation.folder_id
  const cid = await api("create_conversation", {
    folderId,
    agentType: "codex",
    title: "Project | technology conference",
  })
  const state = () =>
    api("get_business_conversation_state", { conversationId: cid })
  const history = async () => {
    try {
      return await api("get_folder_conversation", {
        conversationId: cid,
        userTurnLimit: 25,
      })
    } catch (error) {
      // Prompt admission precedes the native runtime's first rollout flush.
      // Retry only that expected missing-file window, within until's bound.
      if (
        error?.code === "not_found" ||
        /Conversation not found/.test(String(error?.message ?? error))
      )
        return { turns: [] }
      throw error
    }
  }
  const switchTo = async (target) => {
    await page
      .locator("[data-business-agent-control]:visible select")
      .first()
      .selectOption(target)
    return until(
      state,
      (s) => {
        if (s?.switchStatus === "failed") throw Error(s.switchError)
        return s?.activeAgent === target && s.switchStatus === "succeeded"
      },
      `${name}: handoff to ${target}`
    )
  }
  const prompt = (connectionId, text, clientMessageId = randomUUID()) =>
    api("acp_prompt", {
      connectionId,
      folderId,
      conversationId: cid,
      clientMessageId,
      blocks: [{ type: "text", text }],
    })
  const sendUi = async (text, response) => {
    const input = page
      .locator('[contenteditable="true"]')
      .filter({ visible: true })
      .last()
    await input.waitFor({ timeout: 90_000 })
    await input.fill(text)
    await input.press("Enter")
    await until(
      history,
      (d) =>
        d.turns.some(
          (t) => t.role === "user" && JSON.stringify(t.blocks).includes(text)
        ) &&
        d.turns.some(
          (t) =>
            t.role === "assistant" &&
            JSON.stringify(t.blocks).includes(response)
        ),
      `${name}: persisted ${response}`
    )
    await page
      .locator(".chat-scroll-port:visible")
      .last()
      .getByText(response, { exact: false })
      .last()
      .waitFor({ timeout: 90_000 })
  }
  try {
    const initial = await api("acp_connect", {
      agentType: "codex",
      workingDir: f.cwd,
    })
    await until(
      () => api("acp_get_session_snapshot", { connectionId: initial }),
      (s) => s?.selectors_ready && s?.external_id,
      "Initial Codex session"
    )
    await prompt(initial, "BUSINESS_CODEX_FIRST")
    const first = await until(
      history,
      (d) => JSON.stringify(d.turns).includes("BUSINESS_CODEX_COMPLETED"),
      "Original Codex history"
    )
    const originalExternal = first.summary.external_id
    await openBusiness(page, api, cid, folderId, "codex")
    const otherBefore = (await api("list_all_conversations")).find(
      (c) => c.id === conversation.id
    )
    const visibleCount = (await api("list_all_conversations")).length
    const live = await api("acp_restore_conversation", { conversationId: cid })
    await prompt(live.connectionId, "BUSINESS_SLOW_BOUNDARY")
    await page
      .locator("[data-business-agent-control]:visible select")
      .first()
      .selectOption("claude_code")
    const queued = await until(
      state,
      (s) => s?.switchStatus === "queued",
      "Switch waits for the running native turn"
    )
    assert.equal(queued.activeAgent, "codex")
    let current = await until(
      state,
      (s) => {
        if (s?.switchStatus === "failed") throw Error(s.switchError)
        return (
          s?.activeAgent === "claude_code" && s.switchStatus === "succeeded"
        )
      },
      "Queued Claude handoff completes"
    )
    assert.equal(current.sessions.length, 2)
    assert.notEqual(
      current.sessions[0].externalId,
      current.sessions[1].externalId
    )
    assert.equal((await api("list_all_conversations")).length, visibleCount)
    const skill = join(f.cwd, ".agents", "skills", "official-document")
    await mkdir(skill, { recursive: true })
    await writeFile(
      join(skill, "SKILL.md"),
      "BUSINESS_SHARED_SKILL_BODY\nKeep template A and its acceptance rules.\n"
    )
    await api("update_business_task_state", {
      conversationId: cid,
      revision: current.revision,
      task: {
        objective: "BUSINESS_VERIFIED_STATE",
        requirements: ["Keep template A"],
        completed: [],
        pending: ["agenda"],
        latestFiles: ["agenda-v1.md"],
        decisions: [],
      },
    })
    await api("save_business_shared_memory", {
      conversationId: cid,
      item: {
        scope: "task",
        scopeId: cid,
        name: "preferences",
        content: "BUSINESS_VERIFIED_MEMORY",
        source: "user",
        revision: 0,
        updatedAt: "",
      },
    })
    const claudeBefore = claudeModel.evidence().length
    await sendUi(
      "BUSINESS_CLAUDE_USER $official-document",
      "BUSINESS_CLAUDE_COMPLETED agenda-v2.md"
    )
    assert.ok(
      claudeModel
        .evidence()
        .slice(claudeBefore)
        .some(
          (r) => r.taskState && r.memory && r.skill && r.previousAgent && r.user
        ),
      "Actual Claude model request did not receive task state/memory/skill/increment"
    )
    current = await switchTo("codex")
    assert.equal(
      current.sessions.find((s) => s.agentType === "codex").externalId,
      originalExternal,
      "Codex return must resume its own original native session"
    )
    const returned = await api("acp_restore_conversation", {
      conversationId: cid,
    })
    await until(
      () =>
        api("acp_get_goal_state", {
          connectionId: returned.connectionId,
        }),
      (value) => value.actions?.includes("set"),
      "Returned Codex advertises its native Goal action"
    )
    await assert.rejects(
      api("acp_goal_control", {
        connectionId: returned.connectionId,
        conversationId: cid,
        action: "set",
        objective: "BUSINESS_GOAL_MUST_WAIT_FOR_CONTEXT",
      }),
      /Shared business context/,
      "Native Goal must wait for an actual business bridge"
    )
    const codexBefore = goalModel.businessEvidence().length
    await sendUi(
      "BUSINESS_CODEX_RETURN $official-document",
      "BUSINESS_CODEX_COMPLETED agenda-v1.md"
    )
    await until(
      () => goalModel.businessEvidence().slice(codexBefore),
      (rs) =>
        rs.some(
          (r) => r.taskState && r.memory && r.skill && r.previousAgent && r.user
        ),
      "Actual Codex return request includes Claude increment"
    )
    const final = await until(
      history,
      (d) =>
        d.turns.filter(
          (t) =>
            t.role === "user" &&
            JSON.stringify(t.blocks).includes("BUSINESS_CODEX_RETURN")
        ).length === 1 &&
        d.turns.filter((t) => t.role === "assistant").length === 4,
      "Four completed business rounds"
    )
    assert.equal(new Set(final.turns.map((t) => t.id)).size, final.turns.length)
    for (const marker of [
      "BUSINESS_CODEX_FIRST",
      "BUSINESS_SLOW_BOUNDARY",
      "BUSINESS_CLAUDE_USER",
      "BUSINESS_CODEX_RETURN",
    ]) {
      assert.equal(
        final.turns.filter(
          (t) => t.role === "user" && JSON.stringify(t.blocks).includes(marker)
        ).length,
        1,
        `Duplicate user round: ${marker}`
      )
    }
    // Two independent clients race with one CAS revision. Only one may queue.
    current = await state()
    const race = await Promise.allSettled(
      [1, 2].map(() =>
        api("switch_business_agent", {
          request: {
            conversationId: cid,
            requestId: randomUUID(),
            targetAgent: "claude_code",
            expectedRevision: current.revision,
          },
        })
      )
    )
    assert.equal(race.filter((r) => r.status === "fulfilled").length, 1)
    await until(
      state,
      (s) => s?.activeAgent === "claude_code" && s.switchStatus === "succeeded",
      "Concurrent winner activates once"
    )
    current = await state()
    assert.equal(current.sessions.length, 2)
    assert.equal((await api("list_all_conversations")).length, visibleCount)
    const otherAfter = (await api("list_all_conversations")).find(
      (c) => c.id === conversation.id
    )
    assert.equal(otherAfter.agent_type, otherBefore.agent_type)
    assert.equal(otherAfter.external_id, otherBefore.external_id)
    await openBusiness(page, api, cid, folderId, "claude_code")
    const restored = await history()
    assert.deepEqual(
      restored.turns.map((t) => t.id),
      final.turns.map((t) => t.id)
    )
    await page.screenshot({ path: join(output, `${name}-business-agent.png`) })
    report.checks.push({
      name: `${name}-business-agent-roundtrip`,
      passed: true,
      protocol: "Real native runtimes/ACP; synthetic local model providers",
      queuedAtBoundary: true,
      sharedContextActuallyReceived: true,
      nativeGoalWaitsForContext: true,
      nativeSessions: current.sessions.map((s) => ({
        agent: s.agentType,
        externalId: s.externalId,
      })),
      conversationId: cid,
      visibleCountUnchanged: true,
      concurrentCas: true,
      historyAfterReload: true,
      otherBusinessUnaffected: true,
    })
    return {
      cid,
      folderId,
      ids: final.turns.map((t) => t.id),
      sessions: current.sessions,
    }
  } catch (error) {
    await page
      .screenshot({ path: join(output, `${name}-business-failure.png`) })
      .catch(() => {})
    await writeFile(
      join(output, `${name}-business-failure.json`),
      JSON.stringify(
        {
          error: String(error.stack ?? error),
          state: await state().catch(() => null),
          // Every value in this fixture is synthetic; never store native auth.
          history: await history().catch(() => null),
          claude: claudeModel.evidence(),
          codex: goalModel.businessEvidence(),
        },
        null,
        2
      )
    )
    throw error
  } finally {
    goalModel.business(false)
  }
}

export async function restoredBusinessPage(page, api, saved, report, name) {
  if (!saved) return
  const state = await api("get_business_conversation_state", {
    conversationId: saved.cid,
  })
  assert.equal(state.sessions.length, 2)
  assert.deepEqual(
    state.sessions.map((s) => s.externalId),
    saved.sessions.map((s) => s.externalId)
  )
  await openBusiness(page, api, saved.cid, saved.folderId, state.activeAgent)
  const restored = await api("get_folder_conversation", {
    conversationId: saved.cid,
    userTurnLimit: 25,
  })
  assert.deepEqual(
    restored.turns.map((t) => t.id),
    saved.ids
  )
  const live = await api("acp_restore_conversation", {
    conversationId: saved.cid,
  })
  await until(
    () => api("acp_get_session_snapshot", { connectionId: live.connectionId }),
    (s) =>
      s?.selectors_ready &&
      s?.external_id ===
        state.sessions.find(
          (session) => session.agentType === state.activeAgent
        ).externalId,
    `${name}: native session resumes after process restart`
  )
  report.checks.push({ name: `${name}-business-restart-restore`, passed: true })
}
