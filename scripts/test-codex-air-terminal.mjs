// Executes the unmodified published prompt/report methods with controlled
// protocol peers. Fixtures are extracts of the Apache-2.0-licensed npm packages;
// each records the complete bundle's SHA256 for independent verification.
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import vm from "node:vm"
import { test } from "node:test"

const base = new URL("../src-tauri/src/acp/", import.meta.url)
const read = (name) => readFileSync(new URL(name, base), "utf8")
const before = read("codex_air_before.txt")
const after = read("codex_air_after.txt")
const deferred = () => {
  let resolve, reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
const drain = () => new Promise((resolve) => setImmediate(resolve))

function harness(fixture, patched = true) {
  const modern = fixture.version === "2.1.1"
  const patchBefore = modern ? read("codex_air_v2_before.txt") : before
  const patchCapture = modern ? read("codex_air_v2_capture.txt") : ""
  const patchAfter = modern ? read("codex_air_v2_after.txt") : after
  let publicationGate = null
  let reportError = null
  const timers = [],
    reports = [],
    audits = [],
    timeline = []
  const root = deferred(),
    children = deferred()
  const state = {
    sessionId: "session",
    cwd: "/original",
    additionalDirectories: ["/extra"],
    currentModelId: "model",
    supportedReasoningEfforts: [],
    supportedInputModalities: ["text"],
    agentMode: {},
  }
  const activePrompts = new Map(),
    pendingTurnStarts = new Map()
  const handler = new Proxy(
    {
      waitForNativeSubagents: () => children.promise,
      getFailure: () => null,
      takeCompletedPlan: () => null,
      isTurnDiffOversized: () => false,
      getTurnDiff: () => "original-diff",
    },
    { get: (target, key) => target[key] ?? (async () => {}) }
  )
  const context = vm.createContext({
    logger: {
      log: (message, data) => timeline.push({ message, data }),
      error: () => {},
    },
    setTimeout: (callback) => timers.push(callback),
    captureAgentFileChangeWorkspace: (cwd, additionalDirectories) => ({
      cwd,
      additionalDirectories: [...additionalDirectories],
      diffRoot: cwd,
    }),
    createReportedAgentFileChangeReport: (requestId, diff, workspace) => {
      audits.push({ params: { requestId, diff, workspace } })
      if (reportError) throw reportError
      return { requestId, status: "reported", paths: ["result.pdf"] }
    },
    AgentFileChangeReportError: class extends Error {},
    AcpToolCallRenderer: class {},
    clientSupportsCompaction: () => false,
    clientSupportsNotices: () => false,
    clientSupportsAgentFileChangeReports: () => true,
    parseAgentFileChangeReportRequest: (meta) => meta?.request ?? null,
    clientSupportsPlanUpdates: () => false,
    clientSupportsTypedSessionFailures: () => false,
    clientSupportsSubagents: () => true,
    CodexEventHandler: class {
      constructor() {
        return handler
      }
    },
    CodexApprovalHandler: class {},
    CodexElicitationHandler: class {},
    ModelId: { fromString: (id) => id },
    resolveFastServiceTier: () => null,
    RequestError: class extends Error {},
    CODEX_PROCESS_EXITED_ERROR_CODE: -1,
    ACPSessionConnection: class {
      async update(update) {
        reports.push(update._meta.air.payload.report)
        if (publicationGate) await publicationGate.promise
      }
    },
    JETBRAINS_META_KEY: "air",
    AIR_META_KEY: "payload",
    AIR_EXTENSION_VERSION_KEY: "version",
    AIR_EXTENSION_VERSION: 1,
    AIR_AGENT_FILE_CHANGE_REPORT_KEY: "report",
    createUnavailableAgentFileChangeReport: (requestId, reason) => ({
      requestId,
      reason,
      status: "unavailable",
    }),
  })
  const prompt = patched
    ? fixture.prompt
        .replace(patchBefore, patchCapture)
        .replace(
          "      activePrompt.complete();\n    }\n  }\n",
          `      activePrompt.complete();\n${patchAfter}    }\n  }\n`
        )
    : fixture.prompt
  assert.equal(fixture.prompt.split(patchBefore).length, 2)
  const methods = vm.runInContext(`({${prompt},${fixture.publish}})`, context)
  const agent = {
    ...methods,
    providerUpdate: null,
    clientCapabilities: {},
    activePrompts,
    pendingTurnStarts,
    getSessionState: () => state,
    trackActivePrompt(sessionId) {
      assert.equal(
        activePrompts.has(sessionId),
        false,
        "next prompt must be admissible"
      )
      const control = new AbortController()
      const active = {
        signal: control.signal,
        closeSignal: new Promise(() => {}),
        abort: () => control.abort(),
        complete: () => activePrompts.delete(sessionId),
      }
      activePrompts.set(sessionId, active)
      return active
    },
    observePromptRequestCancellation: () => () => {},
    createPendingTurnStart: deferred,
    permissionLifecycleContext: () => ({ beginPrompt: () => ({}) }),
    availableCommands: {
      tryHandleCommand: async () => ({ handled: false }),
      publish: async () => {},
    },
    runWithProcessCheck: (fn) => fn(),
    cancelBeforeTurnStarted: () => new Promise(() => {}),
    sessionIsClosing: () => false,
    promptShouldStop: () => false,
    terminalFailurePromptResponse: () => null,
    buildPromptUsage: () => null,
    buildQuotaMeta: () => ({}),
    publishFallbackSessionTitle: async () => {},
    createPromptFallbackTitle: () => "",
    cancelledPromptResponse: () => ({ stopReason: "cancelled" }),
    codexAcpClient: {
      subscribeToSessionEvents: async () => {},
      waitForSessionNotifications: async () => {},
      sendPrompt: async (...args) => {
        args[7]("turn-old")
        return root.promise
      },
      runAgentFileChangeReport: (params) => {
        const result = deferred()
        audits.push({ params, ...result })
        return result.promise
      },
    },
  }
  const start = (id = "run-old", withAir = true) =>
    agent.prompt({
      sessionId: "session",
      prompt: [{ type: "text", text: "work" }],
      _meta: withAir ? { request: { requestId: id, version: 1 } } : {},
    })
  const completeRoot = (status = "completed") =>
    root.resolve({ turn: { id: "turn-old", status } })
  return {
    agent,
    state,
    reports,
    audits,
    timers,
    timeline,
    start,
    completeRoot,
    children,
    activePrompts,
    handler,
    delayPublication: () => (publicationGate = deferred()),
    failReport: () => (reportError = new Error("file verification failed")),
  }
}

for (const version of ["1.10.0", "1.11.0"]) {
  const fixture = JSON.parse(read(`fixtures/codex-acp-${version}-prompt.json`))
  test(`${version}: AIR starts only after all awaited prompt cleanup has finished`, async () => {
    const h = harness(fixture)
    const cleanup = deferred()
    h.handler.dispose = () => cleanup.promise
    const prompt = h.start()
    h.completeRoot()
    h.children.resolve()
    await drain()
    assert.equal(h.timers.length, 0)
    assert.equal(h.activePrompts.size, 1)
    cleanup.resolve()
    assert.equal((await prompt).stopReason, "end_turn")
    assert.equal(h.activePrompts.size, 0)
    assert.equal(h.timers.length, 1)
  })
  test(`${version}: cancellation wins root completion without starting an audit turn`, async () => {
    const h = harness(fixture)
    const prompt = h.start()
    await drain()
    h.activePrompts.get("session").abort()
    h.completeRoot()
    h.children.resolve()
    assert.equal((await prompt).stopReason, "cancelled")
    h.timers.shift()()
    await drain()
    assert.equal(h.audits.length, 0)
    assert.equal(h.reports[0].requestId, "run-old")
    assert.equal(h.reports[0].reason, "cancelled")
  })

  test(`${version}: no AIR request retains ordinary terminal behavior`, async () => {
    const h = harness(fixture)
    const prompt = h.start("no-air", false)
    h.completeRoot()
    h.children.resolve()
    assert.equal((await prompt).stopReason, "end_turn")
    assert.equal(h.timers.length, 0)
  })
  test(`${version}: immediately resolved AIR publishes once and next prompt completes`, async () => {
    const h = harness(fixture)
    h.agent.codexAcpClient.runAgentFileChangeReport = async (params) => ({
      requestId: params.requestId,
      status: "reported",
      paths: ["result.pdf"],
    })
    const prompt = h.start()
    h.completeRoot()
    h.children.resolve()
    assert.equal((await prompt).stopReason, "end_turn")
    assert.equal(h.reports.length, 0)
    h.timers.shift()()
    await drain()
    assert.equal(h.reports.length, 1)
    assert.equal(h.reports[0].requestId, "run-old")
    assert.equal((await h.start("run-next", false)).stopReason, "end_turn")
    assert.equal(h.reports.length, 1)
    assert.equal(h.activePrompts.size, 0)
  })
  test(`${version}: reproduce upstream terminal blocked by AIR`, async () => {
    const h = harness(fixture, false)
    let done = false
    const prompt = h.start().then((r) => {
      done = true
      return r
    })
    h.completeRoot()
    h.children.resolve()
    await drain()
    assert.equal(done, false)
    assert.equal(h.activePrompts.size, 1)
    assert.equal(h.audits.length, 1)
    h.audits[0].resolve({
      requestId: "run-old",
      status: "unavailable",
      reason: "timeout",
    })
    assert.equal((await prompt).stopReason, "end_turn")
    console.log(
      `${version} before: root completed -> AIR pending [task active] -> AIR timeout -> ACP end_turn`
    )
  })

  for (const outcome of [
    "immediate",
    "delayed",
    "missing",
    "timeout",
    "failure",
    "cancelled",
  ]) {
    test(`${version}: terminal independent of AIR ${outcome}; late result cannot affect next turn`, async () => {
      const h = harness(fixture)
      const prompt = h.start()
      const rootCompletedAt = performance.now()
      h.completeRoot()
      h.children.resolve()
      assert.equal((await prompt).stopReason, "end_turn")
      const terminalLatencyMs = performance.now() - rootCompletedAt
      assert.equal(
        h.audits.length,
        0,
        "audit must start after terminal microtasks"
      )
      assert.equal(h.activePrompts.size, 0)
      const nextRoot = deferred()
      h.agent.codexAcpClient.sendPrompt = async (...args) => {
        args[7]("turn-next")
        return nextRoot.promise
      }
      h.state.cwd = "/next"
      h.state.additionalDirectories.push("/new")
      let nextDone = false
      const next = h.start("run-next", false).then((r) => {
        nextDone = true
        return r
      })
      await drain()
      h.timers.shift()()
      await drain()
      assert.equal(h.audits[0].params.turnId, "turn-old")
      assert.equal(h.audits[0].params.requestId, "run-old")
      assert.equal(h.audits[0].params.workspace.cwd, "/original")
      assert.deepEqual(
        Array.from(h.audits[0].params.workspace.additionalDirectories),
        ["/extra"]
      )
      // Cancelling the new active prompt must not cancel the old audit.
      if (outcome === "cancelled") h.activePrompts.get("session").abort()
      assert.equal(h.audits[0].params.signal.aborted, false)
      if (outcome === "failure") h.audits[0].reject(new Error("audit failure"))
      else if (outcome !== "missing")
        h.audits[0].resolve({
          requestId: "run-old",
          status: ["immediate", "delayed"].includes(outcome)
            ? "reported"
            : "unavailable",
          paths: ["result.pdf"],
          reason: outcome,
        })
      await drain()
      assert.equal(nextDone, false)
      assert.equal(h.state.currentTurnId, "turn-next")
      if (outcome !== "missing") assert.equal(h.reports[0].requestId, "run-old")
      nextRoot.resolve({
        turn: {
          id: "turn-next",
          status: outcome === "cancelled" ? "interrupted" : "completed",
        },
      })
      assert.equal(
        (await next).stopReason,
        outcome === "cancelled" ? "cancelled" : "end_turn"
      )
      console.log(
        `${version} after ${outcome}: root -> ACP end_turn ${terminalLatencyMs.toFixed(3)}ms -> next accepted -> old AIR -> next remains active`
      )
    })
  }
  test(`${version}: root completion/final answer cannot bypass real native child work`, async () => {
    const h = harness(fixture)
    let done = false
    const prompt = h.start().then((r) => {
      done = true
      return r
    })
    await drain()
    assert.equal(done, false)
    h.completeRoot()
    h.completeRoot()
    await drain()
    assert.equal(done, false)
    assert.equal(h.timers.length, 0)
    h.children.resolve()
    assert.equal((await prompt).stopReason, "end_turn")
    assert.equal(
      h.timers.length,
      1,
      "duplicate terminal must not duplicate reports"
    )
  })
}

// 2.1.1's AIR is derived from the native diff; a delayed notification or local
// verification must still never hold the prompt response or reuse new-turn data.
const modernFixture = JSON.parse(read("fixtures/codex-acp-2.1.1-prompt.json"))
for (const outcome of ["immediate", "delayed", "missing", "failed"]) {
  test(`2.1.1: ${outcome} AIR cannot block completion or the next prompt`, async () => {
    const h = harness(modernFixture)
    const gate =
      outcome === "delayed" || outcome === "missing"
        ? h.delayPublication()
        : null
    if (outcome === "failed") h.failReport()
    const prompt = h.start()
    h.completeRoot()
    h.children.resolve()
    assert.equal((await prompt).stopReason, "end_turn")
    assert.equal(h.activePrompts.size, 0)
    assert.equal(h.audits.length, 0)
    assert.equal(h.timers.length, 1)
    // Change the shared session and diff before the old callback executes.
    h.state.cwd = "/next"
    h.state.additionalDirectories.push("/next-extra")
    h.handler.getTurnDiff = () => "next-diff"
    assert.equal((await h.start("run-next", false)).stopReason, "end_turn")
    h.timers.shift()()
    await drain()
    assert.equal(h.audits.length, 1)
    assert.equal(h.audits[0].params.requestId, "run-old")
    assert.equal(h.audits[0].params.diff, "original-diff")
    assert.equal(h.audits[0].params.workspace.cwd, "/original")
    assert.deepEqual(
      [...h.audits[0].params.workspace.additionalDirectories],
      ["/extra"]
    )
    assert.equal(h.reports.length, 1)
    assert.equal(h.reports[0].requestId, "run-old")
    assert.equal(
      h.reports[0].status,
      outcome === "failed" ? "unavailable" : "reported"
    )
    if (outcome === "delayed") gate.resolve()
    await drain()
    assert.equal(h.activePrompts.size, 0)
  })
}
test("2.1.1: cancelling preserves the original request's unavailable report", async () => {
  const h = harness(modernFixture)
  const prompt = h.start()
  await drain()
  h.activePrompts.get("session").abort()
  h.completeRoot("interrupted")
  h.children.resolve()
  assert.equal((await prompt).stopReason, "cancelled")
  assert.equal(h.activePrompts.size, 0)
  h.timers.shift()()
  await drain()
  assert.equal(h.audits.length, 0)
  assert.equal(h.reports[0].requestId, "run-old")
  assert.equal(h.reports[0].reason, "cancelled")
})
test("2.1.1: a final answer cannot bypass outstanding native child tasks", async () => {
  const h = harness(modernFixture)
  let terminal = false
  const prompt = h.start().then((r) => {
    terminal = true
    return r
  })
  h.completeRoot()
  await drain()
  assert.equal(terminal, false)
  assert.equal(h.timers.length, 0)
  h.children.resolve()
  assert.equal((await prompt).stopReason, "end_turn")
})
test("2.1.1: disposal finishes before asynchronous AIR begins", async () => {
  const h = harness(modernFixture)
  const disposal = deferred()
  h.handler.dispose = () => disposal.promise
  let terminal = false
  const prompt = h.start().then((r) => {
    terminal = true
    return r
  })
  h.completeRoot()
  h.children.resolve()
  await drain()
  assert.equal(terminal, false)
  assert.equal(h.timers.length, 0)
  disposal.resolve()
  assert.equal((await prompt).stopReason, "end_turn")
  assert.equal(h.activePrompts.size, 0)
  assert.equal(h.timers.length, 1)
})
