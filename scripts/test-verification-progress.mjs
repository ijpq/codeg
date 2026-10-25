import assert from "node:assert/strict"
import { mkdtemp, readFile, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { verificationProgress } from "./verification-progress.mjs"

test("a hung operation fails with its label and retains bounded diagnostics", async () => {
  const root = await mkdtemp(join(tmpdir(), "codeg-progress-"))
  try {
    const progress = verificationProgress(root, {
      checks: [{ name: "previous-check" }],
    })
    await assert.rejects(
      progress.run("desktop:restore", () => new Promise(() => {}), 20),
      /desktop:restore timed out after 20 ms/
    )
    const events = (
      await readFile(join(root, "verification-progress.log"), "utf8")
    )
      .trim()
      .split("\n")
      .map(JSON.parse)
    assert.deepEqual(
      events.map((event) => event.event),
      ["begin", "failed", "end"]
    )
    assert.equal(events[1].active[0].label, "desktop:restore")
    assert.deepEqual(events[1].completedChecks, ["previous-check"])
    assert.deepEqual(events[2].active, [])
    assert.equal(await progress.run("next", () => 42, 20), 42)
  } finally {
    await rm(root, { recursive: true })
  }
})

test("operation failures retain the original error and a late result cannot pass", async () => {
  const root = await mkdtemp(join(tmpdir(), "codeg-progress-"))
  try {
    const progress = verificationProgress(root, { checks: [] })
    const failure = Error("native startup failed")
    await assert.rejects(
      progress.run("startup", () => Promise.reject(failure)),
      (error) => error === failure
    )
    await assert.rejects(
      progress.run(
        "late",
        () => new Promise((resolve) => setTimeout(resolve, 50)),
        10
      ),
      /late timed out/
    )
  } finally {
    await rm(root, { recursive: true })
  }
})
