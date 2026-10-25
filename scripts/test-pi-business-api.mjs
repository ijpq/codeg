// Isolated actual CodeG server + packaged Pi/MCP. Never attaches to a user instance.
import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { randomBytes } from "node:crypto"
import { createWriteStream } from "node:fs"
import { mkdir, writeFile } from "node:fs/promises"
import { resolve, join } from "node:path"
import { createServer } from "node:net"
import { startPiModel } from "./pi-native-model-fixture.mjs"
import { piCapabilityChecks } from "./pi-capability-checks.mjs"

const args = Object.fromEntries(
  process.argv.slice(2).reduce((p, arg, i, all) => {
    if (arg.startsWith("--")) p.push([arg.slice(2), all[i + 1]])
    return p
  }, [])
)
const root = resolve(args.output)
await mkdir(root, { recursive: false })
const workspace = join(root, "workspace"),
  profile = join(root, "profile"),
  agent = join(root, "pi-agent")
await Promise.all([workspace, profile, agent].map((p) => mkdir(p)))
const model = await startPiModel()
await writeFile(
  join(agent, "models.json"),
  JSON.stringify({
    providers: {
      fixture: {
        baseUrl: model.url,
        api: "openai-responses",
        apiKey: "isolated-synthetic-only",
        models: [
          {
            id: "gpt-5.4",
            name: "Synthetic Pi GPT",
            reasoning: true,
            input: ["text", "image"],
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
    defaultModel: "gpt-5.4",
    defaultThinkingLevel: "medium",
    quietStartup: true,
    retry: { enabled: false },
  })
)
const portReservation = createServer()
await new Promise((done) => portReservation.listen(0, "127.0.0.1", done))
const port = portReservation.address().port
await new Promise((done) => portReservation.close(done))
const token = randomBytes(32).toString("hex")
const env = {
  ...process.env,
  HOME: profile,
  USERPROFILE: profile,
  PI_CODING_AGENT_DIR: agent,
  CODEG_DATA_DIR: join(root, "data"),
  CODEG_HOME: join(root, "data"),
  CODEG_HOST: "127.0.0.1",
  CODEG_PORT: String(port),
  CODEG_TOKEN: token,
  CODEG_STATIC_DIR: resolve("out"),
  XDG_CACHE_HOME: join(root, "cache"),
  XDG_CONFIG_HOME: join(root, "config"),
  XDG_DATA_HOME: join(root, "share"),
}
if (args["office-tools"])
  env.PATH = `${resolve(args["office-tools"])}:${env.PATH ?? ""}`
delete env.CODEG_PI_ACP_PATH
delete env.PI_ACP_PI_COMMAND
const log = createWriteStream(join(root, "server.log"))
const child = spawn(resolve(args.server), [], {
  cwd: workspace,
  env,
  stdio: ["ignore", "pipe", "pipe"],
})
child.stdout.pipe(log)
child.stderr.pipe(log)
const api = async (command, body = {}) => {
  const response = await fetch(`http://127.0.0.1:${port}/api/${command}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: "Bearer " + token,
    },
    body: JSON.stringify(body),
  })
  const result = await response.json()
  if (!response.ok)
    throw Object.assign(Error(result.message ?? JSON.stringify(result)), result)
  return result
}
const report = { checks: [], errors: [] }
try {
  let started = false
  for (let i = 0; i < 300; i++) {
    if (child.exitCode !== null)
      throw Error("Owned test server exited: " + child.exitCode)
    try {
      await api("health")
      started = true
      break
    } catch {
      await new Promise((done) => setTimeout(done, 100))
    }
  }
  assert.ok(started, "Isolated server health")
  const folder = await api("open_folder", { path: workspace })
  await piCapabilityChecks({
    api,
    f: { root, cwd: workspace, env },
    folderId: folder.id,
    piModel: model,
    report,
    name: "linux-server",
    download: async (conversationId, id) => {
      const ticket = await api("create_deliverable_download_ticket", {
        conversationId,
        deliverableIds: [id],
      })
      const response = await fetch(
        new URL(ticket.url, `http://127.0.0.1:${port}`)
      )
      assert.equal(response.status, 200)
      assert.match(await response.text(), /PI_VERIFIED_CONTENT/)
    },
  })
  report.passed = true
} catch (error) {
  report.errors.push(String(error.stack ?? error))
  throw error
} finally {
  const exited = new Promise((done) => child.once("exit", done))
  child.kill()
  const timer = setTimeout(() => child.kill("SIGKILL"), 8000)
  if (child.exitCode === null) await exited
  clearTimeout(timer)
  log.end()
  await model.close()
  report.requests = model.evidence()
  await writeFile(
    join(root, "verification.json"),
    JSON.stringify(report, null, 2)
  )
  console.log(
    JSON.stringify({
      passed: Boolean(report.passed),
      checks: report.checks,
      errors: report.errors,
    })
  )
}
