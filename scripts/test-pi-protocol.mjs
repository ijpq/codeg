// Real pinned pi-acp + Pi, with disposable native/config stores and a local
// deterministic provider. This verifies protocol/context, not account login.
import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { createWriteStream } from "node:fs"
import { mkdir, writeFile } from "node:fs/promises"
import { join, resolve } from "node:path"
import { createInterface } from "node:readline"
import { startBusinessModel } from "./business-model-fixture.mjs"

const args = Object.fromEntries(
  process.argv.slice(2).reduce((pairs, arg, i, all) => {
    if (arg.startsWith("--")) pairs.push([arg.slice(2), all[i + 1]])
    return pairs
  }, [])
)
const root = resolve(args.output)
const tools = resolve(args.tools, "node_modules")
await mkdir(root, { recursive: false })
const workspace = join(root, "workspace")
const profile = join(root, "profile")
const agent = join(root, "pi-agent")
await Promise.all([workspace, profile, agent].map((p) => mkdir(p)))
const model = await startBusinessModel("pi")
await writeFile(
  join(agent, "models.json"),
  JSON.stringify({
    providers: {
      fixture: {
        baseUrl: model.url,
        api: "anthropic-messages",
        apiKey: "isolated-test",
        models: [
          {
            id: "pi-business-model",
            reasoning: false,
            input: ["text"],
            contextWindow: 128000,
            maxTokens: 4096,
          },
        ],
      },
    },
  })
)
await writeFile(
  join(agent, "settings.json"),
  JSON.stringify({
    defaultProvider: "fixture",
    defaultModel: "pi-business-model",
    defaultThinkingLevel: "off",
    quietStartup: true,
  })
)
const env = {
  ...process.env,
  HOME: profile,
  USERPROFILE: profile,
  PI_CODING_AGENT_DIR: agent,
  PI_ACP_ENABLE_EMBEDDED_CONTEXT: "true",
  PI_ACP_PI_COMMAND: join(
    tools,
    ".bin",
    process.platform === "win32" ? "pi.cmd" : "pi"
  ),
}
function adapter(name) {
  const child = spawn(
    process.execPath,
    [
      args.adapter
        ? resolve(args.adapter)
        : join(tools, "pi-acp/dist/index.js"),
    ],
    {
      cwd: workspace,
      env,
      stdio: ["pipe", "pipe", "pipe"],
    }
  )
  const log = createWriteStream(join(root, `${name}.stderr.log`))
  child.stderr.pipe(log)
  const pending = new Map()
  let id = 0
  const input = createInterface({ input: child.stdout })
  input.on("line", (line) => {
    const message = JSON.parse(line)
    const request = pending.get(message.id)
    if (!request) return
    pending.delete(message.id)
    clearTimeout(request.timer)
    if (message.error) request.reject(Error(JSON.stringify(message.error)))
    else request.resolve(message.result)
  })
  child.on("exit", (code) => {
    for (const request of pending.values()) {
      clearTimeout(request.timer)
      request.reject(Error(`pi-acp exited: ${code}`))
    }
    pending.clear()
  })
  return {
    rpc(method, params) {
      return new Promise((resolve, reject) => {
        const requestId = ++id
        const timer = setTimeout(() => {
          pending.delete(requestId)
          reject(Error(`Timed out: ${method}`))
        }, 60000)
        pending.set(requestId, { resolve, reject, timer })
        child.stdin.write(
          JSON.stringify({ jsonrpc: "2.0", id: requestId, method, params }) +
            "\n"
        )
      })
    },
    async close() {
      if (child.exitCode === null) {
        const exited = new Promise((done) => child.once("exit", done))
        child.stdin.end()
        const timer = setTimeout(() => child.kill(), 5000)
        await exited
        clearTimeout(timer)
      }
      input.close()
      log.end()
    },
  }
}
let live
try {
  const initialize = async () => {
    const init = await live.rpc("initialize", {
      protocolVersion: 1,
      clientInfo: { name: "codeg-pi-handoff-check", version: "1" },
      clientCapabilities: {},
    })
    assert.equal(init.agentCapabilities.loadSession, true)
    assert.equal(
      init.agentCapabilities.promptCapabilities.embeddedContext,
      true
    )
    return init
  }
  live = adapter("new")
  const capabilities = await initialize()
  const session = await live.rpc("session/new", {
    cwd: workspace,
    mcpServers: [],
  })
  assert.ok(session.sessionId)
  assert.ok(session.configOptions.some((c) => c.id === "model"))
  const identity = await live.rpc("_pi/identity", {
    sessionId: session.sessionId,
  })
  assert.equal(identity.sessionId, session.sessionId)
  await live.close()
  live = adapter("empty-restore")
  await initialize()
  await live.rpc("session/load", {
    sessionId: session.sessionId,
    cwd: workspace,
    mcpServers: [],
  })
  const prompt = async (text, sessionId = session.sessionId) => {
    const result = await live.rpc("session/prompt", {
      sessionId,
      prompt: [{ type: "text", text }],
    })
    assert.equal(result.stopReason, "end_turn")
  }
  await prompt(
    "BUSINESS_PI_USER BUSINESS_VERIFIED_STATE BUSINESS_VERIFIED_MEMORY BUSINESS_SHARED_SKILL_BODY BUSINESS_CODEX_COMPLETED"
  )
  assert.ok(
    model
      .evidence()
      .some(
        (r) => r.taskState && r.memory && r.skill && r.previousAgent && r.user
      )
  )
  await live.close()
  live = adapter("restore")
  await initialize()
  await live.rpc("session/load", {
    sessionId: session.sessionId,
    cwd: workspace,
    mcpServers: [],
  })
  await prompt("BUSINESS_PI_USER resume the same native business session")
  assert.equal(model.evidence().length, 2)
  assert.ok(
    model.evidence()[1].fromPi,
    "Restored Pi model must receive its own prior native reply"
  )
  const cloned = await live.rpc("session/fork", {
    sessionId: session.sessionId,
    cwd: workspace,
    mcpServers: [],
  })
  assert.notEqual(cloned.sessionId, session.sessionId)
  assert.equal(
    (await live.rpc("_pi/identity", { sessionId: session.sessionId }))
      .sessionId,
    session.sessionId
  )
  await live.rpc("session/resume", {
    sessionId: cloned.sessionId,
    cwd: workspace,
    mcpServers: [],
  })
  await prompt("BUSINESS_PI_USER BRANCH_ONLY_NATIVE_CONTEXT", cloned.sessionId)
  const legacy = join(profile, ".pi", "pi-acp")
  await mkdir(legacy, { recursive: true })
  await writeFile(
    join(legacy, "session-map.json"),
    JSON.stringify({
      version: 1,
      sessions: {
        "legacy-acp-alias": {
          sessionId: "legacy-acp-alias",
          cwd: workspace,
          sessionFile: identity.sessionFile,
        },
      },
    })
  )
  await live.close()
  live = adapter("alias-restore")
  await initialize()
  await live.rpc("session/load", {
    sessionId: "legacy-acp-alias",
    cwd: workspace,
    mcpServers: [],
  })
  const repaired = await live.rpc("_pi/identity", {
    sessionId: "legacy-acp-alias",
  })
  assert.equal(repaired.sessionId, session.sessionId)
  assert.equal(repaired.acpSessionId, "legacy-acp-alias")
  await prompt("BUSINESS_PI_USER safe alias restoration", "legacy-acp-alias")
  const result = {
    passed: true,
    runtime: "pi@1.4.0",
    adapter: "pi-acp@0.0.34-codeg.5",
    sessionId: session.sessionId,
    capabilities,
    requests: model.evidence(),
    nativeResume: true,
    emptyResume: true,
    nativeFork: true,
    sourceBindingUnchanged: true,
    legacyAliasVerified: true,
    deterministicProvider: true,
  }
  await writeFile(
    join(root, "verification.json"),
    JSON.stringify(result, null, 2)
  )
  console.log(JSON.stringify(result))
} finally {
  await live?.close()
  await model.close()
}
