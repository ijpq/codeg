import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { once } from "node:events"
import { mkdtemp, readFile, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import {
  markOwnedProcessStop,
  observeOwnedProcess,
} from "./owned-process-lifecycle.mjs"

async function fixture(action) {
  const output = await mkdtemp(join(tmpdir(), "codeg-process-evidence-"))
  try {
    await action(output, async () =>
      (await readFile(join(output, "owned-process-lifecycle.log"), "utf8"))
        .trim()
        .split("\n")
        .map(JSON.parse)
    )
  } finally {
    await rm(output, { recursive: true })
  }
}

test("an unexpected owned-process exit retains its code and cannot be hidden by cleanup", () =>
  fixture(async (output, events) => {
    const child = spawn(
      process.execPath,
      ["-e", "process.exit(23)", "secret-argument"],
      {
        env: { ...process.env, PRIVATE_TEST_SECRET: "secret-environment" },
        stdio: "ignore",
      }
    )
    observeOwnedProcess(child, { output, name: "server:upgrade" })
    await once(child, "close")
    markOwnedProcessStop(child, "server-cleanup")
    const records = await events()
    assert.equal(records.find((e) => e.event === "exit").code, 23)
    assert.equal(records.find((e) => e.event === "exit").stopReason, null)
    assert.equal(records.at(-1).event, "stop-already-exited")
    assert.ok(!JSON.stringify(records).includes("secret-"))
  }))

test("an intentional owned-process stop is distinguished from a runtime exit", () =>
  fixture(async (output, events) => {
    const child = spawn(process.execPath, ["-e", "setInterval(()=>{},1000)"], {
      stdio: "ignore",
    })
    observeOwnedProcess(child, { output, name: "desktop:restart" })
    markOwnedProcessStop(child, "desktop-restart")
    child.kill()
    await once(child, "close")
    const records = await events()
    assert.equal(
      records.find((e) => e.event === "exit").stopReason,
      "desktop-restart"
    )
    assert.equal(records.filter((e) => e.event === "stop-requested").length, 1)
  }))
