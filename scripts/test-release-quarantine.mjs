import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import vm from "node:vm"
import { test } from "node:test"
import { parse } from "yaml"

const workflow = parse(
  readFileSync(
    process.env.CODEG_RELEASE_WORKFLOW_FIXTURE ??
      new URL("../.github/workflows/release.yml", import.meta.url),
    "utf8"
  )
)
const job = workflow.jobs["quarantine-failed-release"]
const sha = "a".repeat(40)
const tag = "v0.41.3-fix3"

async function run({
  releaseId = "7",
  release = {},
  commit = sha,
  annotated = true,
  result = "failure",
} = {}) {
  assert.ok(job, "A published verification failure must quarantine the release")
  const updates = []
  const script = job.steps[0].with.script
  const context = {
    repo: { owner: "ijpq", repo: "codeg" },
    ref: `refs/tags/${tag}`,
    sha,
    runId: 123,
  }
  const github = {
    rest: {
      git: {
        getRef: async () => ({
          data: {
            object: { type: annotated ? "tag" : "commit", sha: commit },
          },
        }),
        getTag: async () => ({ data: { object: { sha: commit } } }),
      },
      repos: {
        getRelease: async (params) => {
          assert.equal(params.release_id, 7)
          return {
            data: {
              tag_name: tag,
              draft: false,
              body: "Original notes",
              ...release,
            },
          }
        },
        updateRelease: async (params) => updates.push(params),
      },
    },
  }
  await new vm.Script(`(async () => {\n${script}\n})()`).runInContext(
    vm.createContext({
      github,
      context,
      core: { info() {} },
      process: {
        env: { RELEASE_ID: releaseId, VERIFICATION_RESULT: result },
      },
    })
  )
  return updates
}

test("postpublication failure and cancellation trigger independent quarantine", () => {
  assert.ok(job)
  assert.deepEqual(job.needs, [
    "create-draft-release",
    "publish-release",
    "verify-published-assets",
  ])
  assert.match(job.if, /always\(\)/)
  assert.match(job.if, /github\.repository == 'ijpq\/codeg'/)
  assert.match(job.if, /needs\.publish-release\.result == 'success'/)
  assert.match(job.if, /needs\.verify-published-assets\.result != 'success'/)
  assert.equal(job.permissions.contents, "write")
})

for (const annotated of [true, false]) {
  test(`quarantine preserves assets and tag (${annotated ? "annotated" : "lightweight"})`, async () => {
    const updates = await run({ annotated })
    assert.equal(updates.length, 1)
    assert.deepEqual(Object.keys(updates[0]).sort(), [
      "body",
      "make_latest",
      "owner",
      "prerelease",
      "release_id",
      "repo",
    ])
    assert.equal(updates[0].prerelease, true)
    assert.equal(updates[0].make_latest, "false")
    assert.match(updates[0].body, /发布后验收未通过/)
    assert.match(updates[0].body, /actions\/runs\/123/)
    assert.ok(updates[0].body.endsWith("Original notes"))
  })
}

test("repeated quarantine does not duplicate or discard release notes", async () => {
  const body = "> 发布后验收未通过\n\nOriginal notes"
  const updates = await run({ release: { body }, result: "cancelled" })
  assert.equal(updates[0].body, body)
})

for (const [name, options] of [
  ["missing release ID", { releaseId: "" }],
  ["another release", { release: { tag_name: "v0.41.3-fix2" } }],
  ["unpublished draft", { release: { draft: true } }],
  ["changed annotated tag", { commit: "b".repeat(40) }],
  ["changed lightweight tag", { commit: "b".repeat(40), annotated: false }],
]) {
  test(`quarantine refuses ${name}`, async () => {
    await assert.rejects(run(options), /identity|match|no longer/)
  })
}
