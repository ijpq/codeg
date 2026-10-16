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
    ])
      entries.push({
        timestamp: new Date(Date.parse(timestamp) + i * 1000).toISOString(),
        type: "event_msg",
        payload,
      })
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
  app = await startDesktop(installation, f, "upgraded-startup")
  try {
    const conversations = await desktopApi(app.page)("list_all_conversations")
    assert.ok(
      conversations.some((c) => c.id === saved.id && c.external_id === f.id),
      "Upgrade lost existing conversation"
    )
    await actualPage(app.page, "upgraded-frontend", options.baseline !== "true")
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
    await importAndOpen(api, s)
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
    child.kill()
    await new Promise((done) =>
      child.exitCode !== null ? done() : child.once("exit", done)
    )
    serverLog.end()
  }
  report.passed = true
} catch (error) {
  report.errors.push(String(error.stack ?? error))
  throw error
} finally {
  await writeFile(
    join(output, "verification.json"),
    JSON.stringify(report, null, 2)
  )
}
console.log(
  JSON.stringify({ passed: true, checks: report.checks.map((c) => c.name) })
)
