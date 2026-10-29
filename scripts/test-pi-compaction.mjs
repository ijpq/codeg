// Native Pi preflight/compaction reproduction, independent of the frontend.
// Packaged frontend checks live in pi-compaction-page.mjs.
import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { mkdir, writeFile } from "node:fs/promises"
import { resolve, join } from "node:path"
import { createInterface } from "node:readline"
import { startPiModel } from "./pi-native-model-fixture.mjs"
const args = Object.fromEntries(
  process.argv.slice(2).reduce((pairs, value, i, all) => {
    if (value.startsWith("--")) pairs.push([value.slice(2), all[i + 1]])
    return pairs
  }, [])
)
const root = resolve(args.output)
await mkdir(root, { recursive: false })
const agent = join(root, "pi-agent")
await mkdir(agent)
const model = await startPiModel()
await writeFile(
  join(agent, "models.json"),
  JSON.stringify({
    providers: {
      fixture: {
        baseUrl: model.url,
        api: "openai-responses",
        apiKey: "isolated-only",
        models: [
          {
            id: "gpt-5.4",
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
    defaultModel: "gpt-5.4",
    quietStartup: true,
    compaction: {
      enabled: false,
      keepRecentTokens: 2048,
      reserveTokens: 16384,
    },
    retry: { enabled: false },
  })
)
const child = spawn(process.execPath, [resolve(args.adapter)], {
  cwd: root,
  env: {
    ...process.env,
    PI_CODING_AGENT_DIR: agent,
    PI_ACP_PI_COMMAND: resolve(args.pi),
  },
  stdio: ["pipe", "pipe", "pipe"],
})
const pending = new Map(),
  events = []
let id = 0,
  stderr = ""
child.stderr.on("data", (c) => {
  stderr += c.toString()
})
const input = createInterface({ input: child.stdout })
input.on("line", (line) => {
  const value = JSON.parse(line)
  if (value.method === "session/update")
    events.push({ ...value.params, receivedAt: Date.now() })
  const p = pending.get(value.id)
  if (!p) return
  pending.delete(value.id)
  clearTimeout(p.timer)
  if (value.error) p.reject(Error(JSON.stringify(value.error)))
  else p.resolve(value.result)
})
function rpc(method, params) {
  return new Promise((resolve, reject) => {
    const requestId = ++id
    const timer = setTimeout(() => {
      pending.delete(requestId)
      reject(Error("RPC timeout: " + method))
    }, 90000)
    pending.set(requestId, { resolve, reject, timer })
    child.stdin.write(
      JSON.stringify({ jsonrpc: "2.0", id: requestId, method, params }) + "\n"
    )
  })
}
let result, failure
try {
  await rpc("initialize", { protocolVersion: 1, clientCapabilities: {} })
  const session = await rpc("session/new", { cwd: root, mcpServers: [] })
  const prompt = (text) =>
    rpc("session/prompt", {
      sessionId: session.sessionId,
      prompt: [{ type: "text", text }],
    })
  model.tokens(120000)
  for (let round = 0; round < 4; round++)
    await prompt(
      `PI_COMPACTION_HISTORY_${round}\n` +
        "Business requirements, reviewed edits and decisions must remain available.\n".repeat(
          1200
        )
    )
  await prompt("/autocompact on")
  model.tokens(42)
  model.compaction(35000)
  const startedAt = Date.now()
  try {
    result = await prompt(
      "PI_COMPACTION_CONTINUE verify retained requirements and reply once"
    )
  } catch (error) {
    failure = String(error)
  }
  const duration = Date.now() - startedAt
  if (args.expect === "before") {
    assert.match(failure ?? "", /PI_EXECUTION_FAILED/)
    assert.ok(duration >= 29000 && duration < 35000)
    await new Promise((done) => setTimeout(done, 6500))
    assert.ok(
      model.evidence().filter((r) => !r.compaction).length > 4,
      "Old adapter already failed but Pi went on to generate"
    )
  } else {
    assert.equal(result?.stopReason, "end_turn", failure)
    assert.ok(duration > 30000)
    const frames = events
      .map((e) => e.update)
      .filter((e) => e.sessionUpdate === "compaction_update")
    assert.equal(frames.length, 2)
    assert.equal(frames[0].status, "in_progress")
    assert.equal(frames[1].status, "completed")
    assert.equal(frames[0].compactionId, frames[1].compactionId)
    assert.ok(frames[1]._meta.contextCompaction.eventId)
    assert.equal(
      (await prompt("PI_COMPACTION_NEXT round after compaction")).stopReason,
      "end_turn"
    )
    await prompt("/autocompact off")
    model.tokens(120000)
    await prompt(
      "PI_COMPACTION_CANCEL_SEED\n" +
        "Retain the reviewed requirements.\n".repeat(3000)
    )
    await prompt("/autocompact on")
    model.tokens(42)
    const starts = events.filter(
      (e) =>
        e.update.sessionUpdate === "compaction_update" &&
        e.update.status === "in_progress"
    ).length
    const cancellation = prompt(
      "PI_COMPACTION_CANCEL_CONTINUE must not execute after stop"
    )
    const deadline = Date.now() + 10000
    while (
      events.filter(
        (e) =>
          e.update.sessionUpdate === "compaction_update" &&
          e.update.status === "in_progress"
      ).length === starts &&
      Date.now() < deadline
    )
      await new Promise((done) => setTimeout(done, 20))
    assert.ok(Date.now() < deadline, "Native preflight compaction began")
    child.stdin.write(
      JSON.stringify({
        jsonrpc: "2.0",
        method: "session/cancel",
        params: { sessionId: session.sessionId },
      }) + "\n"
    )
    assert.equal((await cancellation).stopReason, "cancelled")
    assert.equal(
      events
        .map((e) => e.update)
        .filter((e) => e.sessionUpdate === "compaction_update")
        .at(-1).status,
      "cancelled"
    )
    assert.equal(
      model.evidence().filter((r) => !r.compaction).length,
      7,
      "Stopped preflight must not start a model generation"
    )
  }
  await writeFile(
    join(root, "verification.json"),
    JSON.stringify(
      {
        passed: true,
        expect: args.expect,
        result,
        failure,
        duration,
        events,
        requests: model.evidence(),
      },
      null,
      2
    )
  )
} finally {
  await writeFile(join(root, "stderr.log"), stderr)
  child.stdin.end()
  const timer = setTimeout(() => child.kill(), 5000)
  if (child.exitCode === null)
    await new Promise((done) => child.once("exit", done))
  clearTimeout(timer)
  input.close()
  await model.close()
}
