// Real Pi + adapter against an isolated Responses endpoint. No account credentials.
import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { createWriteStream } from "node:fs"
import { mkdir, writeFile } from "node:fs/promises"
import { join, resolve } from "node:path"
import { startGoalModel } from "./native-codex-model-fixture.mjs"

const args = Object.fromEntries(
  process.argv.slice(2).reduce((p, a, i, all) => {
    if (a.startsWith("--")) p.push([a.slice(2), all[i + 1]])
    return p
  }, [])
)
const root = resolve(args.output)
await mkdir(root, { recursive: false })
const workspace = join(root, "workspace"),
  profile = join(root, "profile"),
  agent = join(root, "agent")
await Promise.all([workspace, profile, agent].map((p) => mkdir(p)))
const model = await startGoalModel()
model.ordinary(true)
await writeFile(
  join(agent, "models.json"),
  JSON.stringify({
    providers: {
      fixture: {
        baseUrl: model.url,
        api: "openai-responses",
        apiKey: "isolated-test",
        models: ["gpt-5.4", "gpt-5.2", "unknown-model"].map((id) => ({
          id,
          name: id,
          reasoning: true,
          input: ["text"],
          contextWindow: 128000,
          maxTokens: 2048,
        })),
      },
    },
  })
)
await writeFile(
  join(agent, "settings.json"),
  JSON.stringify({
    defaultProvider: "fixture",
    defaultModel: "gpt-5.4",
    defaultThinkingLevel: "medium",
    quietStartup: true,
    retry: { enabled: false },
  })
)
const env = {
  ...process.env,
  HOME: profile,
  USERPROFILE: profile,
  PI_CODING_AGENT_DIR: agent,
  PI_ACP_PI_COMMAND: resolve(args.pi),
  PI_ACP_DATA_DIR: join(root, "adapter-data"),
}
let live
function adapter(label) {
  const child = spawn(process.execPath, [resolve(args.adapter)], {
    cwd: workspace,
    env,
    stdio: ["pipe", "pipe", "pipe"],
  })
  const log = createWriteStream(join(root, label + ".stderr.log"))
  child.stderr.pipe(log)
  const pending = new Map()
  let sequence = 0,
    buffered = ""
  child.stdout.setEncoding("utf8")
  child.stdout.on("data", (chunk) => {
    buffered += chunk
    let boundary
    while ((boundary = buffered.indexOf("\n")) >= 0) {
      const line = buffered.slice(0, boundary)
      buffered = buffered.slice(boundary + 1)
      if (!line.trim()) continue
      const message = JSON.parse(line),
        task = pending.get(message.id)
      if (!task) continue
      clearTimeout(task.timer)
      pending.delete(message.id)
      if (message.error) task.reject(Error(JSON.stringify(message.error)))
      else task.resolve(message.result)
    }
  })
  child.once("exit", () => {
    for (const task of pending.values()) {
      clearTimeout(task.timer)
      task.reject(Error("adapter exited"))
    }
    pending.clear()
  })
  return {
    rpc(method, params) {
      return new Promise((resolve, reject) => {
        const id = ++sequence
        const timer = setTimeout(() => {
          pending.delete(id)
          reject(Error("Timed out: " + method))
        }, 45000)
        pending.set(id, { resolve, reject, timer })
        child.stdin.write(
          JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n"
        )
      })
    },
    async close() {
      if (child.exitCode === null) {
        const exit = new Promise((done) => child.once("exit", done))
        child.stdin.end()
        const timer = setTimeout(() => child.kill(), 3000)
        await exit
        clearTimeout(timer)
      }
      log.end()
    },
  }
}
const tier = (result) =>
  result.configOptions.find((o) => o.id === "service_tier")
const init = () =>
  live.rpc("initialize", {
    protocolVersion: 1,
    clientCapabilities: {},
    clientInfo: { name: "codeg-tier-test", version: "1" },
  })
let sessionId
const setting = (configId, value) =>
  live.rpc("session/set_config_option", { sessionId, configId, value })
const prompt = () =>
  live.rpc("session/prompt", {
    sessionId,
    prompt: [{ type: "text", text: "synthetic tier request" }],
  })
try {
  live = adapter("first")
  const capabilities = await init()
  assert.equal(capabilities._meta.codegPi.serviceTier.configId, "service_tier")
  const session = await live.rpc("session/new", {
    cwd: workspace,
    mcpServers: [],
  })
  sessionId = session.sessionId
  assert.equal(tier(session).currentValue, "off")
  await prompt()
  assert.deepEqual(model.parameters().at(-1), {
    model: "gpt-5.4",
    serviceTier: "default",
    effort: "medium",
  })
  await setting("service_tier", "on")
  model.tier("priority")
  await prompt()
  assert.deepEqual(model.parameters().at(-1), {
    model: "gpt-5.4",
    serviceTier: "priority",
    effort: "medium",
  })
  let state = await live.rpc("session/resume", {
    sessionId,
    cwd: workspace,
    mcpServers: [],
  })
  assert.match(
    tier(state).description,
    /endpoint reported priority; upstream unverified/
  )
  model.tier("missing")
  await prompt()
  state = await live.rpc("session/resume", {
    sessionId,
    cwd: workspace,
    mcpServers: [],
  })
  assert.match(
    tier(state).description,
    /Last request: priority; upstream unverified/
  )
  assert.doesNotMatch(tier(state).description, /endpoint reported/)
  await setting("model", "fixture/gpt-5.2")
  await prompt()
  assert.equal(model.parameters().at(-1).serviceTier, "default")
  await setting("model", "fixture/gpt-5.4")
  state = await live.rpc("session/resume", {
    sessionId,
    cwd: workspace,
    mcpServers: [],
  })
  assert.equal(tier(state).currentValue, "on")
  await setting("model", "fixture/unknown-model")
  await assert.rejects(
    setting("service_tier", "on"),
    /PI_SERVICE_TIER_UNSUPPORTED/
  )
  await prompt()
  assert.equal(model.parameters().at(-1).serviceTier, null)
  await setting("model", "fixture/gpt-5.4")
  model.delay(1200)
  const running = prompt()
  await new Promise((done) => setTimeout(done, 250))
  await assert.rejects(setting("service_tier", "off"), /PI_SETTINGS_BUSY/)
  await running
  assert.equal(model.parameters().at(-1).serviceTier, "priority")
  model.delay(100)
  model.tier("reject")
  await assert.rejects(prompt(), /PI_EXECUTION_FAILED/)
  state = await live.rpc("session/resume", {
    sessionId,
    cwd: workspace,
    mcpServers: [],
  })
  assert.match(tier(state).description, /provider request failed/)
  model.tier("default")
  await live.close()
  live = adapter("restored")
  await init()
  state = await live.rpc("session/load", {
    sessionId,
    cwd: workspace,
    mcpServers: [],
  })
  assert.equal(tier(state).currentValue, "on")
  await prompt()
  state = await live.rpc("session/resume", {
    sessionId,
    cwd: workspace,
    mcpServers: [],
  })
  assert.match(
    tier(state).description,
    /endpoint reported default; upstream unverified/
  )
  await setting("service_tier", "off")
  await prompt()
  assert.equal(model.parameters().at(-1).serviceTier, "default")
  const report = {
    passed: true,
    realPiProtocol: true,
    syntheticProvider: true,
    realAccountVerified: false,
    cases: [
      "off-default",
      "on-priority",
      "reasoning-independent",
      "endpoint-reported",
      "missing-report-unverified",
      "per-model",
      "unsupported-model",
      "busy-rejected",
      "provider-rejection",
      "restore",
      "reported-downgrade",
    ],
    parameters: model.parameters(),
  }
  await writeFile(
    join(root, "verification.json"),
    JSON.stringify(report, null, 2)
  )
  console.log(JSON.stringify(report))
} finally {
  await live?.close()
  await model.close()
}
