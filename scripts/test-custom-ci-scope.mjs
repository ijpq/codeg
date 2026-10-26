import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import { parse } from "yaml"

const workflow = (name) =>
  parse(
    readFileSync(
      new URL(`../.github/workflows/${name}`, import.meta.url),
      "utf8"
    )
  )
const ci = workflow("test.yml")
const release = workflow("release.yml")

function platforms(job, repository) {
  const expression = job.strategy.matrix.os
  const match = expression.match(
    /fromJSON\(github\.repository == '([^']+)' && '([^']+)' \|\| '([^']+)'\)/
  )
  assert.ok(
    match,
    "Platform policy must explicitly distinguish the self-use fork"
  )
  return JSON.parse(repository === match[1] ? match[2] : match[3])
}

test("self-use CI gates only Windows platform builds and preserves upstream", () => {
  assert.deepEqual(platforms(ci.jobs.rust, "ijpq/codeg"), ["windows-latest"])
  assert.deepEqual(platforms(ci.jobs["codex-recovery"], "ijpq/codeg"), [
    "windows-2022",
  ])
  assert.deepEqual(platforms(ci.jobs.rust, "xintaofei/codeg"), [
    "ubuntu-22.04",
    "macos-latest",
    "windows-latest",
  ])
  assert.equal(ci.jobs["install-sh"].if, "github.repository != 'ijpq/codeg'")
  assert.deepEqual(ci.jobs.rust.strategy.matrix.mode, [
    "desktop",
    "server",
    "cli",
  ])
})

test("shared tests remain gates independent of the Linux runner label", () => {
  assert.equal(ci.jobs.frontend["runs-on"], "ubuntu-22.04")
  assert.ok(ci.jobs.frontend.steps.some((step) => step.run === "pnpm test"))
  assert.ok(ci.jobs.frontend.steps.some((step) => step.run === "pnpm build"))
  assert.ok(
    ci.jobs.frontend.steps.some((step) =>
      step.name?.includes("route through commands/*_core")
    )
  )
  const integration = ci.jobs["shared-integration"]
  const command = integration.steps.find((step) => step.run)?.run
  assert.match(command, /--no-default-features/)
  assert.match(command, /--features test-utils/)
  assert.match(command, /--test parsers_snapshot --test api_integration/)
  assert.doesNotMatch(command, /tauri-runtime|cargo build|--release/)
  assert.equal(
    workflow("browser-regression.yml").jobs.browser["runs-on"],
    "ubuntu-22.04"
  )
})

test("Windows still verifies native Plan and Goal and both release packages", () => {
  const native = ci.jobs["codex-recovery"].steps.find((step) =>
    step.name?.includes("Verify native Plan and Goal")
  )
  assert.ok(native)
  assert.equal(
    native.if,
    undefined,
    "Native execution must not remain Linux-only"
  )
  assert.match(native.run, /test-codex-goal-protocol\.py/)
  assert.match(native.run, /suffix="\.exe"/)
  assert.equal(ci.jobs["codex-recovery"].env.PYTHONUTF8, "1")
  for (const name of ["build-tauri", "build-server"]) {
    assert.deepEqual(
      release.jobs[name].strategy.matrix.include.map((entry) => entry.target),
      ["x86_64-pc-windows-msvc"]
    )
  }
  assert.ok(release.jobs["verify-draft-assets"])
  assert.ok(release.jobs["verify-published-assets"])
  const ui = workflow("windows-ui-regression.yml").jobs
  assert.equal(ui["setup-ui"]["runs-on"], "windows-2022")
  assert.ok(
    ui["setup-ui"].steps.some((step) =>
      step.name?.includes("Build isolated candidate Setup and Server")
    )
  )
})
