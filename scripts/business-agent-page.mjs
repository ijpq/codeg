// Exercises the packaged app and real ACP adapters against deterministic local
// model endpoints. No live model login or reasoning is claimed by this check.
import assert from "node:assert/strict"
import { mkdir, writeFile } from "node:fs/promises"
import { join, dirname } from "node:path"
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
export async function openBusiness(page, api, cid, folderId, agent) {
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
  // Draft focus is device-local and deliberately survives a remote tab save.
  // Select this business through its real tab before inspecting its controls,
  // just as actualPage does for an imported conversation.
  const tab = page.locator(
    `[data-tab-id="conv-${folderId}-${agent}-${cid}"] [role="tab"]`
  )
  await tab.waitFor({ timeout: 30_000 })
  await tab.click()
  await until(
    () => tab.getAttribute("aria-selected"),
    (selected) => selected === "true",
    "Requested business tab is actually selected"
  )
  await page
    .locator("[data-business-agent-control]:visible select")
    .first()
    .waitFor({ timeout: 90_000 })
}

export async function unavailableSourcePage({
  page,
  api,
  f,
  folderId,
  goalModel,
  report,
  name,
  output,
}) {
  if (!goalModel) return
  const setting = (await api("acp_list_agents")).find(
    (agent) => agent.agent_type === "codex"
  )
  goalModel.business(true)
  const cid = await api("create_conversation", {
    folderId,
    agentType: "codex",
    title: "Unavailable source recovery",
  })
  const readHistory = async () => {
    try {
      return await api("get_folder_conversation", {
        conversationId: cid,
        userTurnLimit: 25,
      })
    } catch (error) {
      if (
        error?.code === "not_found" ||
        /Conversation not found/.test(String(error?.message ?? error))
      )
        return { turns: [] }
      throw error
    }
  }
  try {
    const connectionId = await api("acp_connect", {
      agentType: "codex",
      workingDir: f.cwd,
    })
    await until(
      () => api("acp_get_session_snapshot", { connectionId }),
      (s) => s.selectors_ready && s.external_id,
      "Fresh Codex prompt ready"
    )
    await api("acp_prompt", {
      connectionId,
      folderId,
      conversationId: cid,
      clientMessageId: randomUUID(),
      blocks: [{ type: "text", text: "BUSINESS_CODEX_FIRST" }],
    })
    const original = await until(
      readHistory,
      (d) => JSON.stringify(d.turns).includes("BUSINESS_CODEX_COMPLETED"),
      "Source history was actually saved"
    )
    await until(
      () => api("acp_get_session_snapshot", { connectionId }),
      (s) => s.status === "connected" && !s.turn_in_flight,
      "Source round has completed before disconnect"
    )
    await api("acp_disconnect", { connectionId })
    const resumed = await api("acp_restore_conversation", {
      conversationId: cid,
    })
    await until(
      () =>
        api("acp_get_session_snapshot", { connectionId: resumed.connectionId }),
      (s) =>
        s.selectors_ready && s.external_id === original.summary.external_id,
      "Default packaged Codex resumes its actual saved native session"
    )
    await api("acp_disconnect", { connectionId: resumed.connectionId })
    await api("acp_update_agent_env", {
      agentType: "codex",
      enabled: true,
      env: {
        ...setting.env,
        CODEG_CODEX_ACP_PATH: join(
          f.root,
          "intentionally-missing-codex-acp.cmd"
        ),
      },
      modelProviderId: setting.model_provider_id ?? null,
    })
    await assert.rejects(
      api("acp_restore_conversation", { conversationId: cid }),
      // The same failed native initialization can be wrapped as ACP Protocol
      // on Windows. Prove rejection after a successful native resume and the
      // deliberately missing launcher; do not depend on localized OS wording.
      /ACP protocol error:|not found|not installed|spawn|cannot|failed|no such|系统找不到/i,
      "Source must actually fail to start"
    )
    await openBusiness(page, api, cid, folderId, "codex")
    const selector = page
      .locator("[data-business-agent-toolbar]:visible select")
      .first()
    assert.equal(
      await selector.isEnabled(),
      true,
      "Source startup failure must leave the Agent switch usable"
    )
    await page.screenshot({
      path: join(output, `${name}-unavailable-source.png`),
    })
    await selector.selectOption("pi")
    const current = await until(
      () => api("get_business_conversation_state", { conversationId: cid }),
      (s) => {
        if (s?.switchStatus === "failed") throw Error(s.switchError)
        return s?.activeAgent === "pi" && s.switchStatus === "succeeded"
      },
      "Switch from unavailable source to verified Pi"
    )
    assert.equal(
      current.sessions.find((s) => s.agentType === "codex").externalId,
      original.summary.external_id
    )
    await until(
      () => selector.inputValue(),
      (v) => v === "pi",
      "Successful offline-source switch is visible without restart"
    )
    const after = await readHistory()
    assert.equal(
      after.turns.filter((t) => t.role === "user").length,
      original.turns.filter((t) => t.role === "user").length
    )
    assert.ok(JSON.stringify(after.turns).includes("BUSINESS_CODEX_COMPLETED"))
    report.checks.push({
      name: `${name}-unavailable-source-switch`,
      passed: true,
      originalSessionRetained: true,
      defaultCodexNewAndResume: true,
      targetVerifiedReady: true,
      historyRetained: true,
      uiUpdatedWithoutRestart: true,
    })
  } catch (error) {
    await page
      .screenshot({
        path: join(output, `${name}-unavailable-source-failure.png`),
      })
      .catch(() => {})
    await writeFile(
      join(output, `${name}-unavailable-source-failure.json`),
      JSON.stringify(
        {
          conversationId: cid,
          path: new URL(page.url()).pathname,
          error: String(error.stack ?? error),
          tabs: await api("list_opened_tabs").catch(() => null),
          business: await api("get_business_conversation_state", {
            conversationId: cid,
          }).catch(() => null),
          body: (await page.locator("body").innerText()).slice(-12000),
        },
        null,
        2
      )
    )
    throw error
  } finally {
    await api("acp_update_agent_env", {
      agentType: "codex",
      enabled: setting.enabled,
      env: setting.env,
      modelProviderId: setting.model_provider_id ?? null,
    })
    goalModel.business(false)
  }
}

export async function businessAgentPage({
  page,
  api,
  f,
  conversation,
  name,
  goalModel,
  claudeModel,
  piModel,
  report,
  output,
}) {
  if (!goalModel || !claudeModel || !piModel) return null
  goalModel.business(true)
  const folderId = conversation.folder_id
  await writeFile(
    join(f.cwd, "AGENTS.md"),
    "BUSINESS_WORKSPACE_RULE: preserve existing business templates and acceptance rules.\n"
  )
  // Only the source engine's private instruction entry knows these rules.
  // Target-native workspace discovery alone cannot satisfy this assertion.
  await writeFile(
    join(f.env.CODEX_HOME, "AGENTS.md"),
    `BUSINESS_SOURCE_INSTANCE_RULES / BUSINESS_SAFE_BOUNDARY: never restart the production Server. Read ${join(f.cwd, "custom-ledger.md")} and its subledger before work. Automatically choose and maintain the existing .agents/skills/official-document/SKILL.md and preserve its acceptance rules.\n`
  )
  await writeFile(
    join(f.cwd, "custom-ledger.md"),
    "Isolated ledger fixture; production instances must remain untouched.\n"
  )
  const cid = await api("create_conversation", {
    folderId,
    agentType: "codex",
    title: "Project | technology conference",
  })
  const state = () =>
    api("get_business_conversation_state", { conversationId: cid })
  const switchResponses = []
  let observer = null
  let businessTitle = null
  const recordSwitch = async (response) => {
    if (!response.url().endsWith("/api/switch_business_agent")) return
    // Only this isolated fixture's synthetic switch parameters and result;
    // never record transport headers or native credentials.
    switchResponses.push({
      request: response.request().postDataJSON(),
      status: response.status(),
      body: await response.json().catch(() => null),
    })
  }
  page.on("response", recordSwitch)
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
    const previous = await state()
    // A programmatic second client may have just committed a handoff. Wait
    // for the tested page to observe that revision before issuing a new one.
    if (previous) {
      await until(
        () =>
          page
            .locator("[data-business-agent-control]:visible select")
            .first()
            .inputValue(),
        (value) => value === previous.activeAgent,
        "Composer observes the current committed Agent"
      )
    }
    await page
      .locator("[data-business-agent-control]:visible select")
      .first()
      .selectOption(target)
    return until(
      state,
      (s) => {
        if (
          s?.switchStatus === "failed" &&
          s.pendingRequestId !== previous?.pendingRequestId
        )
          throw Error(s.switchError)
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
      (d) => {
        const user = d.turns.findIndex(
          (t) => t.role === "user" && JSON.stringify(t.blocks).includes(text)
        )
        return (
          user >= 0 &&
          d.turns
            .slice(user + 1)
            .some(
              (t) =>
                t.role === "assistant" &&
                JSON.stringify(t.blocks).includes(response)
            )
        )
      },
      `${name}: persisted ${response}`
    )
    await page
      .locator(".chat-scroll-port:visible")
      .last()
      .getByText(response, { exact: false })
      .last()
      .waitFor({ timeout: 90_000 })
    if (businessTitle != null)
      assert.equal(
        (await history()).summary.title,
        businessTitle,
        "Native session titles cannot rename the shared business"
      )
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
    const toolbar = page
      .locator("[data-business-agent-toolbar]:visible")
      .first()
    await toolbar.waitFor()
    // A native WebView attached through CDP has no emulated viewport, so
    // viewportSize() is null. Restore its actual dimensions after the narrow
    // check as well, before later desktop-tab navigation and assertions.
    const viewport =
      page.viewportSize() ??
      (await page.evaluate(() => ({
        width: window.innerWidth,
        height: window.innerHeight,
      })))
    try {
      await page.setViewportSize({ width: 390, height: 844 })
      const bounds = await toolbar.boundingBox()
      assert.ok(
        bounds &&
          bounds.width > 150 &&
          bounds.x >= 0 &&
          bounds.x + bounds.width <= 391,
        "Agent toolbar fits a narrow conversation"
      )
      await page.screenshot({
        path: join(output, `${name}-agent-toolbar-narrow.png`),
      })
    } finally {
      await page.setViewportSize(viewport)
    }
    if (name.startsWith("server")) {
      observer = await page.context().newPage()
      await observer.goto(page.url())
      await observer
        .locator("[data-business-agent-toolbar]:visible select")
        .first()
        .waitFor({ timeout: 90_000 })
    }
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
    await assert.rejects(
      api("submit_business_message", {
        request: {
          conversationId: cid,
          clientMessageId: randomUUID(),
          blocks: [
            { type: "text", text: "Must not reach the retiring connection" },
          ],
        },
      }),
      /BUSINESS_SWITCH_PENDING/
    )
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
    await until(
      () =>
        page
          .locator("[data-business-agent-toolbar]:visible select")
          .first()
          .inputValue(),
      (agent) => agent === "claude_code",
      "Initiating page receives committed handoff without reload"
    )
    if (observer) {
      await until(
        () =>
          observer
            .locator("[data-business-agent-toolbar]:visible select")
            .first()
            .inputValue(),
        (agent) => agent === "claude_code",
        "Second Server client receives committed handoff without reload"
      )
    }
    businessTitle = (await history()).summary.title
    assert.ok(
      businessTitle && !businessTitle.includes("<codeg-business-context>")
    )
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
          (r) =>
            r.taskState &&
            r.memory &&
            r.skill &&
            r.previousAgent &&
            r.user &&
            r.rules &&
            r.ledger
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
          (r) =>
            r.taskState &&
            r.memory &&
            r.skill &&
            r.previousAgent &&
            r.user &&
            r.rules &&
            r.ledger
        ),
      "Actual Codex return request includes Claude increment"
    )
    let final = await until(
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
    await api("acp_update_agent_env", {
      agentType: "pi",
      enabled: true,
      env: { PI_ACP_PI_COMMAND: join(f.root, "missing-pi-runtime.cmd") },
      modelProviderId: null,
    })
    await page
      .locator("[data-business-agent-control]:visible select")
      .first()
      .selectOption("pi")
    const failedPi = await until(
      state,
      (s) => s?.switchStatus === "failed",
      "Missing Pi fails preparation"
    )
    assert.equal(failedPi.activeAgent, "codex")
    assert.equal(failedPi.sessions.length, 2)
    assert.match(failedPi.switchError, /Pi is not installed/)
    await until(
      () =>
        page
          .locator("[data-business-agent-control]:visible select")
          .first()
          .isEnabled(),
      Boolean,
      "Failed handoff releases the UI for retry"
    )
    await page
      .locator("[data-business-agent-control]:visible [role=alert]")
      .filter({ hasText: "Pi is not installed" })
      .waitFor()
    assert.equal(
      failedPi.sessions.find((s) => s.agentType === "codex").externalId,
      originalExternal
    )
    await api("acp_update_agent_env", {
      agentType: "pi",
      enabled: true,
      env: {},
      modelProviderId: null,
    })
    current = await switchTo("pi")
    const originalPi = current.sessions.find((s) => s.agentType === "pi")
    assert.ok(originalPi?.externalId)
    assert.equal(current.sessions.length, 3)
    const piConnection = await api("acp_restore_conversation", {
      conversationId: cid,
    })
    await api("acp_set_config_option", {
      connectionId: piConnection.connectionId,
      configId: "service_tier",
      valueId: "on",
    })
    const piBefore = piModel.evidence().length
    await sendUi(
      "BUSINESS_PI_USER_FIRST $official-document",
      "BUSINESS_PI_COMPLETED agenda-v3.md"
    )
    await until(
      () => piModel.evidence().slice(piBefore),
      (rs) =>
        rs.some(
          (r) =>
            r.taskState &&
            r.memory &&
            r.skill &&
            r.previousAgent &&
            r.fromClaude &&
            r.user &&
            r.rules &&
            r.ledger
        ),
      "Actual Pi model receives shared state, Skill and both engines' increments"
    )
    assert.equal(piModel.evidence().at(-1).tier, "priority")
    current = await switchTo("codex")
    assert.equal(
      current.sessions.find((s) => s.agentType === "codex").externalId,
      originalExternal
    )
    const codexAfterPi = goalModel.businessEvidence().length
    await sendUi(
      "BUSINESS_CODEX_RETURN_PI $official-document",
      "BUSINESS_CODEX_COMPLETED agenda-v1.md"
    )
    await until(
      () => goalModel.businessEvidence().slice(codexAfterPi),
      (rs) => rs.some((r) => r.fromPi && r.user && r.rules && r.ledger),
      "Codex receives Pi increment"
    )
    await switchTo("claude_code")
    const claudeAfterPi = claudeModel.evidence().length
    await sendUi(
      "BUSINESS_CLAUDE_USER_AFTER_PI $official-document",
      "BUSINESS_CLAUDE_COMPLETED agenda-v2.md"
    )
    await until(
      () => claudeModel.evidence().slice(claudeAfterPi),
      (rs) => rs.some((r) => r.fromPi && r.user),
      "Claude receives Pi increment"
    )
    current = await switchTo("pi")
    assert.equal(
      current.sessions.find((s) => s.agentType === "pi").externalId,
      originalPi.externalId
    )
    const piReturn = piModel.evidence().length
    await sendUi(
      "BUSINESS_PI_USER_RETURN $official-document",
      "BUSINESS_PI_COMPLETED agenda-v3.md"
    )
    await until(
      () => piModel.evidence().slice(piReturn),
      (rs) => rs.some((r) => r.fromPi && r.fromClaude && r.user),
      "Pi resumes its native context plus new Claude increment"
    )
    assert.equal(piModel.evidence().at(-1).tier, "priority")
    final = await until(
      history,
      (d) => d.turns.filter((t) => t.role === "assistant").length === 8,
      "Eight completed rounds across three engines"
    )
    for (const marker of [
      "BUSINESS_PI_USER_FIRST",
      "BUSINESS_CODEX_RETURN_PI",
      "BUSINESS_CLAUDE_USER_AFTER_PI",
      "BUSINESS_PI_USER_RETURN",
    ]) {
      assert.equal(
        final.turns.filter(
          (t) => t.role === "user" && JSON.stringify(t.blocks).includes(marker)
        ).length,
        1,
        `Duplicate cross-engine user round: ${marker}`
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
    assert.equal(current.sessions.length, 3)
    current = await switchTo("pi")
    assert.equal((await api("list_all_conversations")).length, visibleCount)
    const otherAfter = (await api("list_all_conversations")).find(
      (c) => c.id === conversation.id
    )
    assert.equal(otherAfter.agent_type, otherBefore.agent_type)
    assert.equal(otherAfter.external_id, otherBefore.external_id)
    await openBusiness(page, api, cid, folderId, "pi")
    const restored = await history()
    assert.equal(restored.summary.title, businessTitle)
    assert.deepEqual(
      restored.turns.map((t) => t.id),
      final.turns.map((t) => t.id)
    )
    await page.screenshot({ path: join(output, `${name}-business-agent.png`) })
    const agents = await api("acp_list_agents")
    const piRuntime = agents.find((agent) => agent.agent_type === "pi")
    assert.equal(piRuntime.installed_version, "0.0.34-codeg.2")
    assert.equal(piRuntime.selected_runtime?.source, "bundled")
    assert.match(
      piRuntime.selected_runtime.adapter_path,
      /pi-runtime[\\/]pi-acp\.cmd$/
    )
    assert.equal(
      await api("acp_detect_agent_local_version", { agentType: "pi" }),
      "0.0.34-codeg.2"
    )
    assert.equal(
      (await api("acp_get_agent_status", { agentType: "pi" }))
        .installed_version,
      "0.0.34-codeg.2"
    )
    await assert.rejects(
      api("acp_uninstall_agent", { agentType: "pi", taskId: randomUUID() }),
      /matching CodeG runtime group/,
      "Selected private Pi adapter cannot be overwritten by a global npm action"
    )
    const codexRuntime = agents.find((agent) => agent.agent_type === "codex")
    assert.equal(codexRuntime.selected_runtime?.source, "bundled")
    assert.equal(codexRuntime.selected_runtime.adapter_version, "2.2.2")
    report.checks.push({
      name: `${name}-business-agent-roundtrip`,
      passed: true,
      protocol: "Real native runtimes/ACP; synthetic local model providers",
      queuedAtBoundary: true,
      sharedContextActuallyReceived: true,
      piNativeContextAndIncrement: true,
      piPreparationFailureRetainsOriginal: true,
      piMcpSupported: true,
      piTierRestored: true,
      nativeGoalWaitsForContext: true,
      nativeSessions: current.sessions.map((s) => ({
        agent: s.agentType,
        externalId: s.externalId,
      })),
      conversationId: cid,
      visibleCountUnchanged: true,
      businessTitleStable: true,
      concurrentCas: true,
      historyAfterReload: true,
      otherBusinessUnaffected: true,
      selectedRuntimeVersion: true,
      sourceRulesAndLedgerActuallyReceived: true,
      serverSecondClientSync: Boolean(observer),
      narrowToolbar: true,
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
          switchResponses,
          visibleErrors: await page
            .locator("[data-business-agent-control] [role=alert]")
            .allTextContents()
            .catch(() => []),
          state: await state().catch(() => null),
          // Every value in this fixture is synthetic; never store native auth.
          history: await history().catch(() => null),
          claude: claudeModel.evidence(),
          pi: piModel.evidence(),
          codex: goalModel.businessEvidence(),
        },
        null,
        2
      )
    )
    throw error
  } finally {
    await observer?.close()
    page.off("response", recordSwitch)
    goalModel.business(false)
  }
}

// Delay only the disposable child launcher, then terminate the owned app tree.
// On restart the same durable request launches the actual pinned native ACP.
export async function stageInterruptedHandoff(api, f, saved) {
  if (!saved) return null
  const setting = (await api("acp_list_agents")).find(
    (a) => a.agent_type === "codex"
  )
  const actual = setting.selected_runtime.adapter_path
  const node = join(dirname(actual), "../pi-runtime/node.exe")
  const flag = join(f.root, "release-handoff.flag")
  const wrapper = join(f.root, "delayed-codex-acp.cmd")
  await writeFile(
    wrapper,
    `@echo off\r\nif not exist "${flag}" "${node}" -e "setTimeout(() => {}, 300000)"\r\ncall "${actual}" %*\r\nexit /b %errorlevel%\r\n`
  )
  await api("acp_update_agent_env", {
    agentType: "codex",
    enabled: true,
    env: { ...setting.env, CODEG_CODEX_ACP_PATH: wrapper },
    modelProviderId: setting.model_provider_id ?? null,
  })
  const before = await api("get_business_conversation_state", {
    conversationId: saved.cid,
  })
  assert.equal(before.activeAgent, "pi")
  const requestId = randomUUID()
  await api("switch_business_agent", {
    request: {
      conversationId: saved.cid,
      requestId,
      targetAgent: "codex",
      expectedRevision: before.revision,
    },
  })
  await until(
    () => api("get_business_conversation_state", { conversationId: saved.cid }),
    (s) => {
      if (s?.switchStatus === "failed") throw Error(s.switchError)
      return (
        s?.switchPhase === "verifying_target" && s.switchStatus === "preparing"
      )
    },
    "Persisted target preparation is in progress before process interruption"
  )
  return { flag, requestId, setting, cid: saved.cid, sessions: before.sessions }
}

export async function finishInterruptedHandoff(api, pending, report, name) {
  if (!pending) return
  const state = await until(
    () =>
      api("get_business_conversation_state", { conversationId: pending.cid }),
    (s) => {
      if (s?.switchStatus === "failed") throw Error(s.switchError)
      return s?.switchStatus === "succeeded"
    },
    "Restart automatically resumes the persisted handoff"
  )
  assert.equal(state.pendingRequestId, pending.requestId)
  assert.equal(state.activeAgent, "codex")
  assert.deepEqual(
    state.sessions.map((s) => s.externalId),
    pending.sessions.map((s) => s.externalId)
  )
  await api("acp_update_agent_env", {
    agentType: "codex",
    enabled: pending.setting.enabled,
    env: pending.setting.env,
    modelProviderId: pending.setting.model_provider_id ?? null,
  })
  report.checks.push({
    name: `${name}-interrupted-handoff-auto-recovery`,
    passed: true,
    sameRequest: true,
    sameNativeSessions: true,
    actualTargetReady: true,
  })
}

export async function restoredBusinessPage(page, api, saved, report, name) {
  if (!saved) return
  const state = await api("get_business_conversation_state", {
    conversationId: saved.cid,
  })
  assert.equal(state.sessions.length, 3)
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
