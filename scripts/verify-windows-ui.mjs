// Runs only in disposable Windows CI. Every app, port, profile and data path
// belongs to this invocation; never discovers or controls an existing CodeG.
import { chromium } from "playwright-core"
import { spawn, execFileSync } from "node:child_process"
import { createServer } from "node:http"
import { mkdir, readFile, writeFile } from "node:fs/promises"
import { createWriteStream } from "node:fs"
import { resolve, join, relative, isAbsolute } from "node:path"
import { randomUUID, randomBytes, createHash } from "node:crypto"
import assert from "node:assert/strict"
const options = Object.fromEntries(
  process.argv.slice(2).reduce((pairs, arg, i, all) => {
    if (arg.startsWith("--")) pairs.push([arg.slice(2), all[i + 1]])
    return pairs
  }, [])
)
assert.equal(process.platform, "win32", "Setup verification requires Windows")
assert.equal(process.env.GITHUB_ACTIONS, "true", "Use an isolated CI runner")
const output = resolve(options.output)
const rel = relative(resolve(process.env.RUNNER_TEMP), output)
assert.ok(
  rel && !rel.startsWith("..") && !isAbsolute(rel),
  "Evidence/data must be under RUNNER_TEMP"
)
await mkdir(output, { recursive: true })
const report = { version: options.version, checks: [], errors: [] }
const delay = (ms) => new Promise((done) => setTimeout(done, ms))
async function reservePort() {
  const server = createServer()
  await new Promise((done) => server.listen(0, "127.0.0.1", done))
  const port = server.address().port
  await new Promise((done) => server.close(done))
  return port
}
async function startGoalModel() {
  let requests = 0
  const server = createServer(async (request, response) => {
    if (request.method !== "POST") {
      response.writeHead(404).end()
      return
    }
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    const body = JSON.parse(Buffer.concat(chunks).toString())
    requests++
    const text = "SYNTHETIC NATIVE GOAL RESPONSE"
    const item = {
      id: `msg_${requests}`,
      type: "message",
      role: "assistant",
      status: "completed",
      content: [{ type: "output_text", text, annotations: [] }],
    }
    const result = {
      id: `resp_${requests}`,
      object: "response",
      created_at: 1791763200,
      status: "completed",
      model: body.model,
      output: [item],
      usage: {
        input_tokens: 42,
        output_tokens: 3,
        total_tokens: 45,
        input_tokens_details: { cached_tokens: 0 },
        output_tokens_details: { reasoning_tokens: 0 },
      },
    }
    const events = [
      {
        type: "response.created",
        response: { ...result, status: "in_progress", output: [] },
      },
      {
        type: "response.output_item.added",
        output_index: 0,
        item: { ...item, status: "in_progress", content: [] },
      },
      {
        type: "response.content_part.added",
        item_id: item.id,
        output_index: 0,
        content_index: 0,
        part: { type: "output_text", text: "", annotations: [] },
      },
      {
        type: "response.output_text.delta",
        item_id: item.id,
        output_index: 0,
        content_index: 0,
        delta: text,
      },
      {
        type: "response.output_text.done",
        item_id: item.id,
        output_index: 0,
        content_index: 0,
        text,
      },
      { type: "response.output_item.done", output_index: 0, item },
      { type: "response.completed", response: result },
    ]
    await delay(100)
    if (!response.destroyed) {
      response.writeHead(200, { "Content-Type": "text/event-stream" })
      response.end(
        events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join("")
      )
    }
  })
  await new Promise((done) => server.listen(0, "127.0.0.1", done))
  return {
    url: `http://127.0.0.1:${server.address().port}/v1`,
    count: () => requests,
    close: async () => {
      server.closeAllConnections()
      await new Promise((done) => server.close(done))
    },
  }
}
const goalModel = options["native-tools"] ? await startGoalModel() : null
async function enableNativeAgent(f) {
  if (!goalModel) return
  const pathKey =
    Object.keys(f.env).find((key) => key.toLowerCase() === "path") ?? "PATH"
  f.env[pathKey] = `${resolve(options["native-tools"])};${f.env[pathKey] ?? ""}`
  f.env.CODEX_PATH = join(resolve(options.server), "runtime", "codex.exe")
  await writeFile(
    join(f.env.CODEX_HOME, "config.toml"),
    `model = "gpt-6-astra"
model_provider = "fixture"
approval_policy = "never"
sandbox_mode = "read-only"
[model_providers.fixture]
name = "Isolated release Goal model"
base_url = "${goalModel.url}"
wire_api = "responses"
requires_openai_auth = false
`
  )
}
async function fixture(name) {
  const root = join(output, name)
  const data = join(root, "data"),
    codex = join(root, "codex"),
    cwd = join(root, "workspace")
  const sessions = join(codex, "sessions", "2026", "10", "12")
  await Promise.all(
    [data, sessions, cwd].map((path) => mkdir(path, { recursive: true }))
  )
  const id = randomUUID(),
    timestamp = "2026-10-12T00:00:00Z"
  const entries = [
    {
      timestamp,
      type: "session_meta",
      payload: {
        id,
        timestamp,
        cwd,
        originator: "codeg-release-verification",
        cli_version: "0.162.1",
        source: "cli",
        model_provider: "fixture",
      },
    },
    {
      timestamp,
      type: "turn_context",
      payload: {
        turn_id: randomUUID(),
        cwd,
        approval_policy: "never",
        sandbox_policy: { type: "read-only" },
        model: "gpt-6-astra",
        effort: "medium",
        summary: "auto",
        multi_agent_version: "v1",
      },
    },
  ]
  for (let i = 0; i < 300; i++) {
    const text =
      i === 299
        ? `${Array.from({ length: 120 }, (_, n) => `Long packaged reply line ${n}.`).join("\n\n")}\n\nRELEASE_FINAL_LINE`
        : `Archived answer ${i}`
    for (const payload of [
      { type: "task_started", turn_id: `turn-${i}` },
      {
        type: "user_message",
        message: `Isolated release test ${i}`,
        images: [],
      },
      { type: "agent_message", message: text, phase: "final_answer" },
      { type: "task_complete", turn_id: `turn-${i}`, last_agent_message: text },
    ]) {
      entries.push({
        timestamp: new Date(Date.parse(timestamp) + i * 1000).toISOString(),
        type: "event_msg",
        payload,
      })
      if (payload.type === "user_message" || payload.type === "agent_message") {
        const user = payload.type === "user_message"
        entries.push({
          timestamp,
          type: "response_item",
          payload: {
            type: "message",
            role: user ? "user" : "assistant",
            ...(!user ? { phase: "final_answer" } : {}),
            content: [
              {
                type: user ? "input_text" : "output_text",
                text: user ? payload.message : text,
              },
            ],
          },
        })
      }
    }
  }
  const rollout = join(sessions, `rollout-2026-10-12T00-00-00-${id}.jsonl`)
  await writeFile(
    rollout,
    entries.map((e) => JSON.stringify(e)).join("\n") + "\n"
  )
  await writeFile(join(data, "upgrade-retention-marker.txt"), id)
  return {
    root,
    data,
    id,
    cwd,
    rollout,
    env: {
      ...process.env,
      CODEG_DATA_DIR: data,
      CODEG_HOME: data,
      CODEX_HOME: codex,
      XDG_CONFIG_HOME: join(root, "config"),
      XDG_DATA_HOME: join(root, "share"),
      XDG_CACHE_HOME: join(root, "cache"),
      WEBVIEW2_USER_DATA_FOLDER: join(root, "webview"),
    },
  }
}
async function run(command, args, env = process.env) {
  const child = spawn(command, args, { env, stdio: "inherit" })
  await new Promise((done, fail) => {
    child.on("error", fail)
    child.on("exit", (code) =>
      code === 0 ? done() : fail(Error(`${command} exited ${code}`))
    )
  })
}
async function install(setup, path) {
  await run(resolve(setup), ["/S", `/D=${path}`])
  assert.ok((await readFile(join(path, "codeg.exe"))).length > 1_000_000)
}
async function stopOwnedProcess(child) {
  if (child.exitCode !== null || child.signalCode !== null) return
  // WebView2 children can exit while taskkill enumerates the tree. Windows
  // then returns 128 even after killing the owned root. Observe that process's
  // actual exit rather than a stale exitCode during execFileSync's blocked tick.
  await new Promise((done, fail) => {
    let killError
    const onExit = () => {
      clearTimeout(timer)
      done()
    }
    const timer = setTimeout(() => {
      child.removeListener("exit", onExit)
      fail(killError ?? Error(`Owned process ${child.pid} did not exit`))
    }, 10_000)
    child.once("exit", onExit)
    try {
      execFileSync("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"])
    } catch (error) {
      killError = error
    }
  })
}
async function startDesktop(installDir, f, name) {
  const port = await reservePort()
  const env = {
    ...f.env,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  }
  const log = createWriteStream(join(output, `${name}.log`))
  const child = spawn(join(installDir, "codeg.exe"), [], {
    env,
    cwd: installDir,
    stdio: ["ignore", "pipe", "pipe"],
  })
  child.stdout.pipe(log)
  child.stderr.pipe(log)
  let browser
  try {
    for (let i = 0; i < 120; i++) {
      if (child.exitCode !== null)
        throw Error(`${name} quit with ${child.exitCode}`)
      try {
        const response = await fetch(`http://127.0.0.1:${port}/json/version`)
        if (response.ok) break
      } catch {}
      await delay(500)
    }
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, {
      timeout: 30_000,
    })
    const context = browser.contexts()[0]
    let page
    for (let i = 0; i < 60; i++) {
      page = context
        .pages()
        .find((p) => /tauri\.localhost|localhost/.test(p.url()))
      if (page) break
      await delay(500)
    }
    assert.ok(page, "Installed app did not create its frontend")
    await page.waitForFunction(
      () => Boolean(window.__TAURI_INTERNALS__?.invoke),
      { timeout: 30_000 }
    )
    return { child, browser, page, port, log }
  } catch (error) {
    await browser?.close().catch(() => {})
    if (child.pid) await stopOwnedProcess(child)
    log.end()
    throw error
  }
}
async function stopDesktop(app) {
  await app.browser.close().catch(() => {})
  await stopOwnedProcess(app.child)
  await delay(1000)
  app.log.end()
}
function desktopApi(page) {
  return (command, args = {}) =>
    page.evaluate(
      ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args),
      { command, args }
    )
}
async function importAndOpen(api, f) {
  await api("scan_importable_sessions")
  const result = await api("import_selected_sessions", {
    selections: [{ agentType: "codex", externalId: f.id }],
  })
  assert.equal(result.failed, 0)
  assert.ok(
    result.imported + result.updated + result.skipped > 0,
    JSON.stringify(result)
  )
  const conversations = await api("list_all_conversations")
  const conversation = conversations.find((c) => c.external_id === f.id)
  assert.ok(conversation, "Fixture session was not imported")
  if (goalModel)
    await api("update_folder_default_agent", {
      folderId: conversation.folder_id,
      defaultAgentType: "codex",
    })
  const tabs = await api("list_opened_tabs")
  const saved = await api("save_opened_tabs", {
    expectedVersion: tabs.version,
    origin: "isolated-release-verification",
    items: [
      {
        id: 0,
        folder_id: conversation.folder_id,
        conversation_id: conversation.id,
        agent_type: "codex",
        position: 0,
        is_active: true,
        is_pinned: false,
      },
    ],
  })
  assert.equal(saved.accepted, true)
  const detail = await api("get_folder_conversation", {
    conversationId: conversation.id,
    userTurnLimit: 25,
  })
  report.checks.push({
    name: "import-fixture",
    conversation,
    tabSave: saved,
    turns: detail.turns?.length,
    hasFinal: JSON.stringify(detail).includes("RELEASE_FINAL_LINE"),
  })
  return conversation
}
async function actualPage(page, name, verifyScroll = true) {
  const errors = []
  page.on("pageerror", (error) => errors.push(String(error)))
  await page.reload()
  // A device-local draft may retain focus across reloads. Select the imported
  // conversation through its real tab, as a reader would, before inspecting it.
  const tab = page.locator('[data-tab-id^="conv-"] [role="tab"]').first()
  await tab.waitFor({ timeout: 30_000 })
  await tab.click()
  try {
    await page
      .getByText("RELEASE_FINAL_LINE", { exact: true })
      .waitFor({ timeout: 60_000 })
  } catch (error) {
    await page.screenshot({ path: join(output, `${name}-failure.png`) })
    await writeFile(
      join(output, `${name}-failure.json`),
      JSON.stringify(
        {
          origin: new URL(page.url()).origin,
          path: new URL(page.url()).pathname,
          body: (await page.locator("body").innerText()).slice(-12000),
          errors,
          tabs: await page
            .evaluate(() =>
              window.__TAURI_INTERNALS__?.invoke("list_opened_tabs")
            )
            .catch(() => null),
        },
        null,
        2
      )
    )
    throw error
  }
  const viewport = page.locator(".chat-scroll-port").last()
  await viewport.evaluate((el) => {
    el.scrollTop = el.scrollHeight
  })
  await page.waitForTimeout(1800)
  const samples = []
  for (let i = 0; i < 12; i++) {
    samples.push(
      await viewport.evaluate((el) => ({
        top: el.scrollTop,
        gap: el.scrollHeight - el.clientHeight - el.scrollTop,
        rows: el.querySelectorAll("[data-message-thread-row]").length,
      }))
    )
    await page.waitForTimeout(60)
  }
  if (verifyScroll) {
    assert.ok(
      Math.max(...samples.map((s) => s.top)) -
        Math.min(...samples.map((s) => s.top)) <=
        1,
      `${name}: completed answer keeps moving`
    )
    assert.ok(samples.at(-1).gap <= 2, `${name}: final answer not at bottom`)
    assert.ok(samples.at(-1).rows < 80, `${name}: virtualization missing`)
    const line = await page
      .getByText("RELEASE_FINAL_LINE", { exact: true })
      .boundingBox()
    const box = await viewport.boundingBox()
    assert.ok(
      line.y + line.height <= box.y + box.height + 1,
      `${name}: final line clipped`
    )
    await viewport.hover()
    await page.mouse.wheel(0, -450)
    await page.waitForTimeout(300)
    const before = await viewport.evaluate((el) => el.scrollTop)
    await page.waitForTimeout(1000)
    const after = await viewport.evaluate((el) => el.scrollTop)
    assert.ok(Math.abs(before - after) <= 2, `${name}: reader pulled down`)
  }
  await page.screenshot({ path: join(output, `${name}.png`) })
  // Errors are retained in full and asserted; console warnings are separate.
  assert.deepEqual(errors, [], `${name}: frontend runtime errors`)
  report.checks.push({ name, samples, errors })
}
async function nativeGoalPage(page, api, f, conversation, name) {
  if (!goalModel) return
  const until = async (read, accepts, label) => {
    let value
    for (let i = 0; i < 600; i++) {
      value = await read()
      if (accepts(value)) return value
      await delay(100)
    }
    throw Error(`${name}: ${label}: ${JSON.stringify(value)}`)
  }
  const find = () =>
    api("acp_find_connection_for_conversation", {
      conversationId: conversation.id,
      sessionId: f.id,
      agentType: "codex",
    })
  try {
    const trigger = page.getByRole("button", {
      name: /^(Execution|执行方式|執行方式):/,
    })
    await trigger.waitFor({ timeout: 90_000 })
    const connection = await until(
      find,
      (value) => value?.connection_id,
      "live connection"
    )
    const connectionId = connection.connection_id
    const goal = () => api("acp_get_goal_state", { connectionId })
    const mode = async () =>
      (
        await api("acp_get_session_snapshot", { connectionId })
      )?.config_options?.find((option) => option.id === "collaboration_mode")
        ?.kind.current_value
    await until(
      goal,
      (value) =>
        ["set", "pause", "resume", "clear"].every((a) =>
          value.actions?.includes(a)
        ),
      "verified native Goal capability"
    )
    await trigger.click()
    const dialog = page.getByRole("dialog").filter({
      has: page.getByRole("radiogroup", {
        name: /^(Execution|执行方式|執行方式)$/,
      }),
    })
    const choose = async (name) => {
      const radio = dialog.getByRole("radio", { name })
      await until(
        () => radio.isEnabled(),
        Boolean,
        "execution selector enabled"
      )
      await radio.click()
    }
    const status = (expected) =>
      until(goal, (value) => value.goal?.status === expected, expected)
    const quiet = async () => {
      await delay(600)
      const count = goalModel.count()
      await delay(1500)
      assert.equal(
        goalModel.count(),
        count,
        `${name}: stopped Goal kept requesting work`
      )
      return count
    }
    const work = (count) =>
      until(
        () => goalModel.count(),
        (value) => value > count,
        "native model work"
      )
    await choose(/^(Plan first|先规划|先規劃)$/)
    await until(mode, (value) => value === "plan", "native Plan selected")
    await choose(/^(Execute goal|目标执行|目標執行)$/)
    const objective = `${name}: native Goal ${f.id}`
    await dialog.locator("textarea").fill(objective)
    let count = goalModel.count()
    await dialog
      .getByRole("button", { name: /^(Start goal|开始目标|開始目標)$/ })
      .click()
    assert.equal((await status("active")).goal.objective, objective)
    await until(
      mode,
      (value) => value === "default",
      "Goal uses native execution mode"
    )
    await work(count)
    await page.screenshot({ path: join(output, `${name}-goal-active.png`) })
    await choose(/^(Execute directly|直接执行|直接執行)$/)
    await status("paused")
    await until(
      mode,
      (value) => value === "default",
      "switch pauses before Direct"
    )
    count = await quiet()
    await choose(/^(Execute goal|目标执行|目標執行)$/)
    await delay(1000)
    assert.equal(
      goalModel.count(),
      count,
      "Selecting Goal resumed it implicitly"
    )
    await status("paused")
    const resume = () =>
      dialog.getByRole("button", { name: /^(Resume|继续|繼續)$/ }).click()
    await resume()
    await status("active")
    await work(count)
    await dialog.getByRole("button", { name: /^(Pause|暂停|暫停)$/ }).click()
    await status("paused")
    count = await quiet()
    await resume()
    await status("active")
    await work(count)
    await dialog
      .getByRole("button", { name: /^(End goal|结束目标|結束目標)$/ })
      .click()
    await until(goal, (value) => value.goal === null, "native Goal cleared")
    await quiet()
    await page.screenshot({ path: join(output, `${name}-goal-ended.png`) })
    await page.keyboard.press("Escape")
    report.checks.push({
      name: `${name}-native-goal-controls`,
      passed: true,
      nativePlan: true,
      startsRealWork: true,
      switchPauses: true,
      explicitResume: true,
      pauseAndEndStopWork: true,
      modelRequests: goalModel.count(),
    })
  } catch (error) {
    await page
      .screenshot({ path: join(output, `${name}-goal-failure.png`) })
      .catch(() => {})
    report.checks.push({
      name: `${name}-native-goal-failure`,
      page: (
        await page
          .locator("body")
          .innerText()
          .catch(() => "unavailable")
      ).slice(-12000),
    })
    throw error
  }
}

async function nativeDraftGoalPage(page, api, name) {
  if (!goalModel) return
  const before = new Set(
    (await api("list_all_conversations")).map((row) => row.id)
  )
  const until = async (read, accepts, label) => {
    let value
    for (let i = 0; i < 600; i++) {
      value = await read()
      if (accepts(value)) return value
      await delay(100)
    }
    throw Error(`${name}: draft ${label}: ${JSON.stringify(value)}`)
  }
  try {
    await page
      .locator(".tab-strip-tail")
      .getByRole("button", {
        name: /^(New Conversation|新建会话|新增會話)$/,
      })
      .first()
      .click()
    const trigger = () =>
      page
        .getByRole("button", {
          name: /^(Execution|执行方式|執行方式):/,
        })
        .last()
    await trigger().waitFor({ timeout: 90_000 })
    await trigger().click()
    const dialog = () =>
      page.getByRole("dialog").filter({
        has: page.getByRole("radiogroup", {
          name: /^(Execution|执行方式|執行方式)$/,
        }),
      })
    const goalChoice = dialog().getByRole("radio", {
      name: /^(Execute goal|目标执行|目標執行)$/,
    })
    await until(() => goalChoice.isEnabled(), Boolean, "Goal capability")
    await goalChoice.click()
    const objective = `${name}: first Goal ${randomUUID()}`
    await dialog().locator("textarea").fill(objective)
    await dialog()
      .getByRole("button", { name: /^(Start goal|开始目标|開始目標)$/ })
      .click()
    const row = await until(
      () => api("list_all_conversations"),
      (rows) => rows.some((row) => !before.has(row.id) && row.external_id),
      "durable conversation"
    ).then((rows) => rows.find((row) => !before.has(row.id) && row.external_id))
    const connection = await until(
      () =>
        api("acp_find_connection_for_conversation", {
          conversationId: row.id,
          agentType: "codex",
        }),
      (value) => value?.connection_id,
      "bound live connection"
    )
    const goal = () =>
      api("acp_get_goal_state", { connectionId: connection.connection_id })
    await until(
      goal,
      (value) => value.goal?.objective === objective,
      "native objective"
    )
    await page
      .getByText("SYNTHETIC NATIVE GOAL RESPONSE", { exact: true })
      .first()
      .waitFor({ timeout: 60_000 })
    // The welcome composer hands off to the transcript's dock after binding.
    // That replacement may close its popover; use the new real composer.
    if (!(await dialog().isVisible())) await trigger().click()
    await dialog()
      .getByRole("button", { name: /^(Pause|暂停|暫停)$/ })
      .click()
    await until(goal, (value) => value.goal?.status === "paused", "pause")
    await page.keyboard.press("Escape")
    await page
      .getByText("SYNTHETIC NATIVE GOAL RESPONSE", { exact: true })
      .first()
      .waitFor({ timeout: 60_000 })
    const detail = await until(
      () =>
        api("get_folder_conversation", {
          conversationId: row.id,
          userTurnLimit: 25,
        }),
      (value) =>
        JSON.stringify(value).includes("SYNTHETIC NATIVE GOAL RESPONSE"),
      "persisted native output"
    )
    assert.equal(detail.summary.external_id, row.external_id)
    await trigger().click()
    await dialog()
      .getByRole("button", { name: /^(End goal|结束目标|結束目標)$/ })
      .click()
    await until(goal, (value) => value.goal === null, "ended")
    await page.keyboard.press("Escape")
    await page.reload()
    const restoredTab = page.locator(
      `[data-tab-id="conv-${row.folder_id}-codex-${row.id}"] [role="tab"]`
    )
    await restoredTab.waitFor({ timeout: 30_000 })
    await restoredTab.click()
    await page
      .getByText("SYNTHETIC NATIVE GOAL RESPONSE", { exact: true })
      .first()
      .waitFor({ timeout: 60_000 })
    await page.screenshot({
      path: join(output, `${name}-draft-goal-restored.png`),
    })
    report.checks.push({
      name: `${name}-draft-native-goal-history`,
      passed: true,
      conversationId: row.id,
      nativeSessionId: row.external_id,
      outputVisible: true,
      restoredAfterReload: true,
    })
  } catch (error) {
    await page
      .screenshot({ path: join(output, `${name}-draft-goal-failure.png`) })
      .catch(() => {})
    throw error
  }
}
try {
  const f = await fixture("setup-upgrade")
  const installation = join(output, "upgraded-installation")
  await install(options.previous, installation)
  let app = await startDesktop(installation, f, "previous-startup")
  let saved
  try {
    saved = await importAndOpen(desktopApi(app.page), f)
    await actualPage(app.page, "previous-frontend", false)
  } finally {
    await stopDesktop(app)
  }
  const original = createHash("sha256")
    .update(await readFile(f.rollout))
    .digest("hex")
  await install(options.setup, installation)
  assert.equal(
    await readFile(join(f.data, "upgrade-retention-marker.txt"), "utf8"),
    f.id
  )
  assert.equal(
    createHash("sha256")
      .update(await readFile(f.rollout))
      .digest("hex"),
    original
  )
  await enableNativeAgent(f)
  app = await startDesktop(installation, f, "upgraded-startup")
  try {
    const conversations = await desktopApi(app.page)("list_all_conversations")
    assert.ok(
      conversations.some((c) => c.id === saved.id && c.external_id === f.id),
      "Upgrade lost existing conversation"
    )
    await actualPage(app.page, "upgraded-frontend", options.baseline !== "true")
    await nativeGoalPage(app.page, desktopApi(app.page), f, saved, "setup")
    await nativeDraftGoalPage(app.page, desktopApi(app.page), "setup")
    if (options.baseline !== "true") {
      const migration = JSON.parse(
        execFileSync(
          "python",
          ["-B", "scripts/release-resources.py", "database", f.data],
          { encoding: "utf8" }
        )
      )
      report.checks.push({ name: "setup-migrations", ...migration })
    }
    await run(process.execPath, ["scripts/browser-scroll-regression.mjs"], {
      ...process.env,
      CODEG_TEST_WEBVIEW_CDP: `http://127.0.0.1:${app.port}`,
      CODEG_BROWSER_EVIDENCE: join(output, "webview-scroll"),
    })
  } finally {
    await stopDesktop(app)
  }
  report.checks.push({
    name: "setup-install-start-upgrade-data-retention",
    passed: true,
  })
  const s = await fixture("server-web")
  await enableNativeAgent(s)
  const port = await reservePort(),
    token = randomBytes(24).toString("hex")
  const env = {
    ...s.env,
    CODEG_HOST: "127.0.0.1",
    CODEG_PORT: String(port),
    CODEG_TOKEN: token,
    CODEG_STATIC_DIR: join(resolve(options.server), "web"),
  }
  const serverLog = createWriteStream(join(output, "server-ui.log"))
  const child = spawn(join(resolve(options.server), "codeg-server.exe"), [], {
    env,
    cwd: resolve(options.server),
    stdio: ["ignore", "pipe", "pipe"],
  })
  child.stdout.pipe(serverLog)
  child.stderr.pipe(serverLog)
  let browser
  try {
    const api = async (command, args = {}) => {
      const response = await fetch(`http://127.0.0.1:${port}/api/${command}`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${token}`,
        },
        body: JSON.stringify(args),
      })
      if (!response.ok)
        throw Error(`${command}: ${response.status} ${await response.text()}`)
      return response.json()
    }
    for (let i = 0; i < 120; i++) {
      if (child.exitCode !== null) throw Error("Isolated server quit")
      try {
        await api("health")
        break
      } catch {}
      await delay(500)
    }
    const conversation = await importAndOpen(api, s)
    browser = await chromium.launch({ channel: "msedge", headless: true })
    const context = await browser.newContext({
      viewport: { width: 1200, height: 900 },
    })
    await context.addInitScript(
      (token) => localStorage.setItem("codeg_token", token),
      token
    )
    const page = await context.newPage()
    await page.goto(`http://127.0.0.1:${port}/workspace`)
    await actualPage(page, "server-web-frontend", options.baseline !== "true")
    await nativeGoalPage(page, api, s, conversation, "server-web")
    await nativeDraftGoalPage(page, api, "server-web")
    if (options.baseline !== "true")
      report.checks.push({
        name: "server-migrations",
        ...JSON.parse(
          execFileSync(
            "python",
            ["-B", "scripts/release-resources.py", "database", s.data],
            { encoding: "utf8" }
          )
        ),
      })
  } finally {
    await browser?.close()
    await stopOwnedProcess(child)
    serverLog.end()
  }
  report.passed = true
} catch (error) {
  report.errors.push(String(error.stack ?? error))
  throw error
} finally {
  await goalModel?.close()
  await writeFile(
    join(output, "verification.json"),
    JSON.stringify(report, null, 2)
  )
}
console.log(
  JSON.stringify({ passed: true, checks: report.checks.map((c) => c.name) })
)
