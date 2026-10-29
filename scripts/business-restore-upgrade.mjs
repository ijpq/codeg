// Create real sessions with the previous packages, then exercise the upgraded
// composer. Only disposable CI profiles and the packages' own runtimes are used.
import assert from "node:assert/strict"
import { randomUUID } from "node:crypto"
import { access, appendFile, writeFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { openBusiness } from "./business-agent-page.mjs"
import { businessComposer, reloadBusiness } from "./business-composer-page.mjs"

const pause = (ms) => new Promise((done) => setTimeout(done, ms))
async function until(read, accepts, label) {
  let value
  const deadline = Date.now() + 90_000
  while (Date.now() < deadline) {
    value = await read()
    if (accepts(value)) return value
    await pause(100)
  }
  throw Error(`${label}: ${JSON.stringify(value)}`)
}

async function history(api, cid) {
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

function hasReply(history, marker, agent) {
  const index = history.turns.findIndex(
    (turn) =>
      turn.role === "user" && JSON.stringify(turn.blocks).includes(marker)
  )
  if (index < 0) return false
  const nextUser = history.turns.findIndex(
    (turn, n) => n > index && turn.role === "user"
  )
  return history.turns
    .slice(index + 1, nextUser < 0 ? undefined : nextUser)
    .some(
      (turn) =>
        turn.role === "assistant" &&
        JSON.stringify(turn.blocks).includes(
          `BUSINESS_${agent.toUpperCase()}_COMPLETED`
        )
    )
}

async function waitIdle(api, cid, agent, sessionId) {
  const live = await api("acp_find_connection_for_conversation", {
    conversationId: cid,
    agentType: agent,
    sessionId,
  })
  assert.ok(live?.connection_id, "The actual business connection exists")
  await until(
    () => api("acp_get_session_snapshot", { connectionId: live.connection_id }),
    (s) => s?.status === "connected" && !s.turn_in_flight && s.selectors_ready,
    "Actual reply has ended before the next composer action"
  )
  return live
}

export async function prepareLegacyPiBusiness({ api, f, folderId, goalModel }) {
  goalModel.business(true)
  try {
    const cid = await api("create_conversation", {
      folderId,
      agentType: "codex",
      title: "Legacy upgrade business already on Pi",
    })
    const connectionId = await api("acp_connect", {
      agentType: "codex",
      workingDir: f.cwd,
    })
    await until(
      () => api("acp_get_session_snapshot", { connectionId }),
      (s) => s?.selectors_ready && s?.external_id,
      "Previous package's native Codex ready for fixture preparation"
    )
    const first = `LEGACY_CODEX_${randomUUID()}`
    await api("acp_prompt", {
      connectionId,
      folderId,
      conversationId: cid,
      clientMessageId: randomUUID(),
      blocks: [{ type: "text", text: first }],
    })
    await until(
      () => history(api, cid),
      (h) => hasReply(h, first, "codex"),
      "Previous native Codex actually replied"
    )
    await until(
      () => api("acp_get_session_snapshot", { connectionId }),
      (s) => s?.status === "connected" && !s.turn_in_flight,
      "Previous Codex turn ended before handoff"
    )
    const prior = await api("get_business_conversation_state", {
      conversationId: cid,
    })
    await api("switch_business_agent", {
      request: {
        conversationId: cid,
        targetAgent: "pi",
        requestId: randomUUID(),
        expectedRevision: prior?.revision ?? null,
      },
    })
    const state = await until(
      () => api("get_business_conversation_state", { conversationId: cid }),
      (s) => {
        if (s?.switchStatus === "failed") throw Error(s.switchError)
        return s?.activeAgent === "pi" && s.switchStatus === "succeeded"
      },
      "Previous package persisted Codex to Pi handoff"
    )
    const pi = await api("acp_restore_conversation", { conversationId: cid })
    const second = `LEGACY_PI_${randomUUID()}`
    await api("acp_prompt", {
      connectionId: pi.connectionId,
      folderId,
      conversationId: cid,
      clientMessageId: randomUUID(),
      blocks: [{ type: "text", text: second }],
    })
    await until(
      () => history(api, cid),
      (h) => hasReply(h, second, "pi"),
      "Previous native Pi actually replied"
    )
    await waitIdle(api, cid, "pi", pi.externalSessionId)
    const saved = await history(api, cid)
    return {
      cid,
      folderId,
      sessions: state.sessions.map(({ agentType, externalId }) => ({
        agentType,
        externalId,
      })),
      historyIds: saved.turns.map((turn) => turn.id),
    }
  } finally {
    goalModel.business(false)
  }
}

export async function legacyPiComposerPage({
  page,
  api,
  f,
  saved,
  goalModel,
  piModel,
  report,
  name,
  output,
  restarted = false,
}) {
  goalModel.business(true)
  const settings = (await api("acp_list_agents")).find(
    (agent) => agent.agent_type === "pi"
  )
  assert.equal(settings.selected_runtime?.source, "bundled")
  assert.equal(settings.selected_runtime?.adapter_version, "0.0.34-codeg.4")
  const updatePi = (env) =>
    api("acp_update_agent_env", {
      agentType: "pi",
      enabled: settings.enabled,
      env,
      modelProviderId: settings.model_provider_id ?? null,
    })
  const read = () => history(api, saved.cid)
  const readState = () =>
    api("get_business_conversation_state", { conversationId: saved.cid })
  const markers = []
  const agents = new Map()
  const actualRequests = (marker, agent) =>
    (agent === "pi" ? piModel.evidence() : goalModel.businessEvidence()).filter(
      (request) => request.composerMarker === marker
    )
  const release = join(f.root, "legacy-restore-release.flag")
  const readinessLog = join(
    output,
    `${name}-legacy-pi-${restarted ? "restart" : "upgrade"}-readiness.log`
  )
  // Read the gate the installed page actually computes. Do not reconstruct
  // React state from disk bundles or assume ACP ready means business-ready.
  // Persist identity/state only; queue drafts and credentials never enter logs.
  const readPageState = () =>
    page.evaluate((cid) => {
      const toolbar = Array.from(
        document.querySelectorAll("[data-session-readiness]")
      ).find(
        (element) =>
          element.getClientRects().length > 0 &&
          JSON.parse(element.getAttribute("data-session-readiness"))
            .conversationId === cid
      )
      return {
        readiness: toolbar
          ? JSON.parse(toolbar.getAttribute("data-session-readiness"))
          : null,
        queue: JSON.parse(
          localStorage.getItem(`codeg:msg-queue:v1:${cid}`) || "[]"
        ).map(({ id, clientMessageId, state, intent }) => ({
          id,
          clientMessageId,
          state,
          intent,
        })),
      }
    }, saved.cid)
  const recordPageState = async (phase, snapshot) => {
    const value = snapshot ?? (await readPageState())
    await appendFile(
      readinessLog,
      JSON.stringify({ at: new Date().toISOString(), phase, ...value }) + "\n"
    )
    return value
  }
  try {
    const state = await readState()
    assert.equal(state.activeAgent, "pi")
    assert.deepEqual(
      state.sessions.map(({ agentType, externalId }) => ({
        agentType,
        externalId,
      })),
      saved.sessions,
      "Upgrade/restart preserves each native binding"
    )
    for (const id of saved.historyIds)
      assert.ok((await read()).turns.some((turn) => turn.id === id))
    if (!restarted) {
      // This adapter delays startup only; it then calls the exact installed
      // Pi ACP and Node. No protocol response or ready state is fabricated.
      const actual = settings.selected_runtime.adapter_path
      const node = join(dirname(actual), "node.exe")
      const blocker = join(f.root, "legacy-restore-delay.cjs")
      const wrapper = join(f.root, "legacy-restore-delay.cmd")
      await access(actual)
      await access(node)
      await writeFile(
        blocker,
        `const fs=require("node:fs"); const end=Date.now()+45000; const timer=setInterval(()=>{if(fs.existsSync(${JSON.stringify(release)})){clearInterval(timer)}else if(Date.now()>end){process.exit(1)}},100);\n`
      )
      await writeFile(
        wrapper,
        `@echo off\r\nif "%~1"=="--version" goto launch\r\n"${node}" "${blocker}"\r\nif errorlevel 1 exit /b 1\r\n:launch\r\ncall "${actual}" %*\r\nexit /b %errorlevel%\r\n`
      )
      await updatePi({ ...settings.env, CODEG_PI_ACP_PATH: wrapper })
    }
    await openBusiness(page, api, saved.cid, saved.folderId, "pi")
    await recordPageState("opened-business")
    const send = async (agent, label, hold = false) => {
      const marker = `LEGACY_COMPOSER_${label}_${randomUUID()}`
      markers.push(marker)
      agents.set(marker, agent)
      const input = await businessComposer(page, {
        conversationId: saved.cid,
        agent,
      })
      await recordPageState(`${label}:before-input`)
      await input.fill(marker)
      await input.press("Enter")
      if (hold) {
        const pending = await until(
          readPageState,
          ({ readiness, queue }) =>
            readiness &&
            queue.length === 1 &&
            queue[0].state === "waiting_session_restore",
          "Input during old Pi recovery is retained in its real durable queue"
        )
        await recordPageState(`${label}:waiting`, pending)
        assert.equal(pending.readiness.connectionReady, false)
        assert.ok(pending.queue[0].clientMessageId)
        await pause(5000)
        const retained = await recordPageState(`${label}:before-release`)
        assert.equal(retained.readiness.connectionReady, false)
        assert.equal(retained.queue.length, 1)
        assert.equal(retained.queue[0].state, "waiting_session_restore")
        assert.equal(
          retained.queue[0].clientMessageId,
          pending.queue[0].clientMessageId,
          "Recovery retains the same client message identity until delivery"
        )
        assert.equal(hasReply(await read(), marker, agent), false)
        assert.equal(actualRequests(marker, agent).length, 0)
        assert.equal(
          actualRequests(marker, agent === "pi" ? "codex" : "pi").length,
          0
        )
        await writeFile(release, "release native packaged ACP")
      }
      await until(
        read,
        (h) => hasReply(h, marker, agent),
        `${name}: upgraded business ${label} actually replies`
      )
      await waitIdle(
        api,
        saved.cid,
        agent,
        saved.sessions.find((s) => s.agentType === agent).externalId
      )
      await until(
        readPageState,
        ({ readiness, queue }) =>
          readiness?.connectionReady && queue.length === 0,
        "Completed input restores the real page gate and clears its queue"
      )
      const completed = await recordPageState(`${label}:completed`)
      assert.equal(completed.readiness.promptReady, true)
      assert.equal(completed.readiness.restoredIdentityReady, true)
      assert.equal(completed.readiness.selectedAgent, agent)
      assert.equal(completed.readiness.connectedAgent, agent)
      assert.equal(completed.readiness.conversationId, saved.cid)
      assert.equal(completed.readiness.connectedConversationId, saved.cid)
      assert.equal(
        completed.readiness.connectedSessionId,
        saved.sessions.find((s) => s.agentType === agent).externalId
      )
      assert.equal(
        completed.readiness.connectedWorkingDir,
        completed.readiness.expectedWorkingDir
      )
      assert.equal(completed.readiness.webConnectionState, "connected")
      const current = await read()
      assert.equal(
        actualRequests(marker, agent).length,
        1,
        "The selected packaged engine sent this exact input to its model once"
      )
      assert.equal(
        actualRequests(marker, agent === "pi" ? "codex" : "pi").length,
        0,
        "No other engine receives this composer message"
      )
      assert.equal(
        current.turns.filter(
          (t) => t.role === "user" && JSON.stringify(t.blocks).includes(marker)
        ).length,
        1
      )
    }
    await send(
      "pi",
      restarted ? "restart-first" : "queued-during-restore",
      !restarted
    )
    if (!restarted) await updatePi(settings.env)
    await send("pi", "second-round")
    if (!restarted) {
      // A separate persisted conversation changes focus without warming this
      // business through a programmatic restore call.
      const other = await api("create_conversation", {
        folderId: saved.folderId,
        agentType: "pi",
        title: "Legacy focus detour",
      })
      const tabs = await api("list_opened_tabs")
      const tabSave = await api("save_opened_tabs", {
        expectedVersion: tabs.version,
        origin: "isolated-legacy-navigation",
        items: [saved.cid, other].map((cid, index) => ({
          id: 0,
          folder_id: saved.folderId,
          conversation_id: cid,
          agent_type: "pi",
          position: index,
          is_active: index === 1,
          is_pinned: false,
        })),
      })
      assert.equal(tabSave.accepted, true)
      // Exercise in-place tab teardown/reattach too. Reloading between these
      // clicks would conceal stale subscription callbacks in the installed UI.
      for (const cid of [other, saved.cid]) {
        const tab = page.locator(
          `[data-tab-id="conv-${saved.folderId}-pi-${cid}"] [role="tab"]`
        )
        await tab.waitFor({ timeout: 30_000 })
        await tab.click()
        await until(
          () => tab.getAttribute("aria-selected"),
          (selected) => selected === "true",
          "Business changes focus without reloading the app"
        )
      }
      await send("pi", "after-tab-switch")
      await reloadBusiness(page, api, {
        conversationId: saved.cid,
        folderId: saved.folderId,
        agent: "pi",
      })
      await send("pi", "after-reload")
      const live = await api("acp_find_connection_for_conversation", {
        conversationId: saved.cid,
        agentType: "pi",
        sessionId: saved.sessions.find((s) => s.agentType === "pi").externalId,
      })
      assert.ok(live?.connection_id)
      await api("acp_disconnect", { connectionId: live.connection_id })
      await send("pi", "after-connection-loss")
      for (const agent of ["codex", "pi"]) {
        await page
          .locator("[data-business-agent-control]:visible select")
          .first()
          .selectOption(agent)
        await until(
          readState,
          (s) => {
            if (s?.switchStatus === "failed") throw Error(s.switchError)
            return s?.activeAgent === agent && s.switchStatus === "succeeded"
          },
          `Upgraded old business hands off to ${agent}`
        )
        await until(
          () =>
            page
              .locator("[data-business-agent-control]:visible select")
              .first()
              .inputValue(),
          (selected) => selected === agent,
          "The actual composer follows the completed handoff"
        )
        await send(agent, `handoff-${agent}`)
      }
    }
    for (const marker of markers) {
      assert.equal(
        (await read()).turns.filter(
          (t) => t.role === "user" && JSON.stringify(t.blocks).includes(marker)
        ).length,
        1
      )
      assert.equal(actualRequests(marker, agents.get(marker)).length, 1)
    }
    const after = await readState()
    assert.deepEqual(
      after.sessions.map(({ agentType, externalId }) => ({
        agentType,
        externalId,
      })),
      saved.sessions
    )
    report.checks.push({
      name: `${name}-legacy-pi-business-${restarted ? "restart" : "upgrade"}-composer`,
      passed: true,
      actualComposer: true,
      protocolRequestMatched: true,
      actualPageReadinessRecorded: true,
      readinessLog,
      oldDataPreparedBy: "v0.40.0-fix6 native Codex and Pi",
      packagedRuntime: true,
      restoredQueueActuallyReplied: !restarted,
      noDuplicateUserRounds: true,
      completedQueueCleared: true,
      actualTabNavigation: !restarted,
      originalBindingsRetained: true,
      rounds: markers.length,
      delayedNativeStartupMs: restarted ? 0 : 5000,
      syntheticProvider: true,
      liveModelAccount: false,
    })
  } catch (error) {
    const pageState = await recordPageState("failure").catch(() => null)
    await page
      .screenshot({
        path: join(output, `${name}-legacy-pi-business-failure.png`),
      })
      .catch(() => {})
    const state = await readState().catch(() => null)
    const connection = await api("acp_find_connection_for_conversation", {
      conversationId: saved.cid,
      agentType: state?.activeAgent ?? "pi",
      sessionId: state?.sessions?.find((s) => s.agentType === state.activeAgent)
        ?.externalId,
    }).catch(() => null)
    const snapshot = connection?.connection_id
      ? await api("acp_get_session_snapshot", {
          connectionId: connection.connection_id,
        }).catch(() => null)
      : null
    await writeFile(
      join(output, `${name}-legacy-pi-business-failure.json`),
      JSON.stringify(
        {
          error: String(error.stack ?? error),
          state,
          selectedRuntime: settings.selected_runtime,
          connection,
          snapshot: snapshot && {
            connection_id: snapshot.connection_id,
            external_id: snapshot.external_id,
            conversation_id: snapshot.conversation_id,
            status: snapshot.status,
            selectors_ready: snapshot.selectors_ready,
            turn_in_flight: snapshot.turn_in_flight,
          },
          pageState,
          userTurns: (await read().catch(() => null))?.turns?.filter(
            (t) => t.role === "user"
          ).length,
        },
        null,
        2
      )
    )
    throw error
  } finally {
    await writeFile(release, "release any remaining isolated startup")
    await updatePi(settings.env)
    goalModel.business(false)
  }
}
