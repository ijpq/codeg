import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { readFile } from "node:fs/promises"
import { join, resolve } from "node:path"

export async function verifyOfficeTestRuntime(runtime) {
  const actual = createHash("sha256")
    .update(await readFile(runtime.path))
    .digest("hex")
  assert.equal(actual, runtime.sha256, "Isolated Office converter changed")
}

export async function prepareOfficeTestRuntime(env, tools) {
  const provenance = JSON.parse(
    await readFile(join(tools, "officecli-test-provenance.json"), "utf8")
  )
  assert.equal(provenance.isolated_test_converter, true)
  assert.match(provenance.sha256, /^[a-f0-9]{64}$/)
  const runtime = {
    path: resolve(
      tools,
      process.platform === "win32" ? "officecli.exe" : "officecli"
    ),
    version: provenance.version,
    sha256: provenance.sha256,
  }
  await verifyOfficeTestRuntime(runtime)
  // OfficeCLI's own documented guards keep the checksum-pinned test input
  // fixed. Change only the fixture's children, never the caller or user config.
  env.OFFICECLI_SKIP_UPDATE = "1"
  env.OFFICECLI_NO_AUTO_INSTALL = "1"
  return runtime
}
