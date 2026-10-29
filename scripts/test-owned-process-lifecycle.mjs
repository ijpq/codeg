import assert from "node:assert/strict"
import { execFileSync, spawn } from "node:child_process"
import { once } from "node:events"
import { createWriteStream } from "node:fs"
import { copyFile, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import {
  captureOwnedProcessLog,
  interruptOwnedProcess,
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

test("closing stdout first cannot hide the later fatal stderr tail", () =>
  fixture(async (output) => {
    const child = spawn(
      process.execPath,
      [
        "-e",
        'process.stdout.end("ordinary-output\\n"); setTimeout(()=>process.stderr.end("fatal-stderr-tail\\n"),40)',
      ],
      { stdio: ["ignore", "pipe", "pipe"] }
    )
    observeOwnedProcess(child, { output, name: "server:stderr-tail" })
    const path = join(output, "runtime.log")
    const flush = captureOwnedProcessLog(child, createWriteStream(path))
    await once(child, "exit")
    await flush()
    assert.match(
      await readFile(path, "utf8"),
      /ordinary-output\nfatal-stderr-tail\n/
    )
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

test(
  "Windows abrupt interruption prevents child EOF from committing a failed handoff",
  {
    skip: process.platform !== "win32" || process.env.GITHUB_ACTIONS !== "true",
  },
  async () => {
    const output = await mkdtemp(
      join(process.env.RUNNER_TEMP, "codeg-owned-interruption-")
    )
    const executable = join(output, "isolated-node.exe")
    await copyFile(process.execPath, executable)
    const fixturePath = join(output, "writer.cjs")
    await writeFile(
      fixturePath,
      `const {spawn} = require("node:child_process");
const fs = require("node:fs");
const child = spawn(process.execPath, ["-e", "setInterval(()=>{},1000)"], {stdio:"ignore"});
child.on("exit", () => fs.writeFileSync(process.argv[3], "failed"));
child.on("spawn", () => fs.writeFileSync(process.argv[2], JSON.stringify({pid:child.pid})));
setInterval(()=>{},1000);
`
    )
    const roots = []
    const readEventually = async (path) => {
      for (let i = 0; i < 100; i++) {
        try {
          return await readFile(path, "utf8")
        } catch (error) {
          if (error.code !== "ENOENT") throw error
        }
        await new Promise((done) => setTimeout(done, 100))
      }
      throw Error("Owned process fixture did not reach the requested boundary")
    }
    const start = (label) => {
      const ready = join(output, label + "-ready.json")
      const failed = join(output, label + "-failed.txt")
      const child = spawn(executable, [fixturePath, ready, failed], {
        stdio: "ignore",
      })
      roots.push(child)
      observeOwnedProcess(child, { output, name: "server:" + label })
      return { child, ready, failed }
    }
    try {
      // Prove the fixture really persists a failure if the child dies while its
      // parent writer remains alive, as the old taskkill /T injection allowed.
      const before = start("child-first")
      const descendant = JSON.parse(await readEventually(before.ready))
      execFileSync("taskkill.exe", ["/PID", String(descendant.pid), "/T", "/F"])
      assert.equal(await readEventually(before.failed), "failed")

      const after = start("app-interruption")
      await readEventually(after.ready)
      markOwnedProcessStop(after.child, "interrupted-handoff-regression")
      interruptOwnedProcess(after.child)
      await once(after.child, "close")
      assert.equal(
        await readFile(after.failed, "utf8").catch((error) => {
          if (error.code === "ENOENT") return null
          throw error
        }),
        null,
        "The abruptly interrupted app must not commit a child-failure result"
      )
      const events = (
        await readFile(join(output, "owned-process-lifecycle.log"), "utf8")
      )
        .trim()
        .split("\n")
        .map(JSON.parse)
      assert.equal(
        events.find((entry) => entry.event === "abrupt-root-interruption")
          .suspendedRoot,
        true
      )
    } finally {
      for (const child of roots) {
        if (child.exitCode === null && child.signalCode === null) {
          markOwnedProcessStop(child, "interruption-fixture-cleanup")
          const closed = once(child, "close")
          execFileSync("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"])
          await closed
        }
      }
      await rm(output, { recursive: true })
    }
  }
)
