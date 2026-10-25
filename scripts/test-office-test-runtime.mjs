import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import {
  prepareOfficeTestRuntime,
  verifyOfficeTestRuntime,
} from "./office-test-runtime.mjs"

test("pinned converter stays isolated and subsequent replacement is rejected", async () => {
  const root = await mkdtemp(join(tmpdir(), "codeg-office-pin-"))
  const parent = { ...process.env }
  const env = { ...parent }
  const path = join(
    root,
    process.platform === "win32" ? "officecli.exe" : "officecli"
  )
  const bytes = Buffer.from("immutable test executable")
  try {
    await writeFile(path, bytes)
    await writeFile(
      join(root, "officecli-test-provenance.json"),
      JSON.stringify({
        version: "v1.0.158",
        sha256: createHash("sha256").update(bytes).digest("hex"),
        isolated_test_converter: true,
      })
    )
    const runtime = await prepareOfficeTestRuntime(env, root)
    assert.equal(env.OFFICECLI_SKIP_UPDATE, "1")
    assert.equal(env.OFFICECLI_NO_AUTO_INSTALL, "1")
    assert.deepEqual({ ...process.env }, parent)
    await verifyOfficeTestRuntime(runtime)
    await writeFile(path, "unexpected replacement")
    await assert.rejects(verifyOfficeTestRuntime(runtime), /converter changed/)
    await assert.rejects(
      prepareOfficeTestRuntime(env, root),
      /converter changed/
    )
  } finally {
    await rm(root, { recursive: true })
  }
})
