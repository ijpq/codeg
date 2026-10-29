// Execute the real release action against GitHub's documented draft visibility
// contract. REST tag lookup is published-only; push-scoped listings include drafts.
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import vm from "node:vm"
import { test } from "node:test"
import "./test-release-history-check.mjs"
import "./test-runtime-page-errors.mjs"
import "./test-release-harness-lint.mjs"

const workflow = readFileSync(
  process.env.CODEG_RELEASE_WORKFLOW_FIXTURE ??
    new URL("../.github/workflows/release.yml", import.meta.url),
  "utf8"
)
const lines = workflow.split("\n")
const first = lines.indexOf("          script: |") + 1
assert.ok(first > 0, "release action script exists")
const body = []
for (const line of lines.slice(first)) {
  if (line && !line.startsWith("            ")) break
  body.push(line.slice(12))
}
const source = `(async () => {\n${body.join("\n")}\n})()`
const tag = "v0.34.0-fix2"
const stored = (draft = true) => ({
  id: 7,
  tag_name: tag,
  draft,
  prerelease: false,
  name: `codeg ${tag}`,
  body: "release notes",
  html_url: "https://github.com/ijpq/codeg/releases/tag/" + tag,
})

async function run({ existing = null, serverOnly = false, listError } = {}) {
  const creates = [],
    updates = [],
    errors = []
  const outputs = {}
  let lists = 0
  const listReleases = () => {}
  const notFound = () => Object.assign(new Error("not found"), { status: 404 })
  const github = {
    paginate: async (method, params) => {
      lists += 1
      assert.equal(method, listReleases)
      assert.equal(params.owner, "ijpq")
      assert.equal(params.repo, "codeg")
      if (listError) throw listError
      return existing ? [existing] : []
    },
    rest: {
      git: {
        getRef: async () => ({
          data: { object: { type: "commit", sha: "a".repeat(40) } },
        }),
      },
      repos: {
        listReleases,
        getCommit: async () => ({ data: { commit: { message: "fallback" } } }),
        getReleaseByTag: async () => {
          if (!existing || existing.draft) throw notFound()
          return { data: existing }
        },
        createRelease: async (params) => {
          creates.push(params)
          if (existing) throw new Error("duplicate release: HTTP 422")
          return { data: { ...stored(), ...params, id: 9 } }
        },
        updateRelease: async (params) => {
          updates.push(params)
          return { data: { ...existing, ...params } }
        },
      },
    },
  }
  await new vm.Script(source).runInContext(
    vm.createContext({
      github,
      context: {
        repo: { owner: "ijpq", repo: "codeg" },
        payload: { repository: { fork: false } },
      },
      core: {
        setFailed: (error) => errors.push(error),
        info: () => {},
        setOutput: (key, value) => (outputs[key] = value),
      },
      process: {
        env: {
          RELEASE_TAG: tag,
          PRERELEASE: "false",
          SERVER_ONLY: String(serverOnly),
        },
      },
      require: (name) => {
        assert.equal(name, "fs")
        return { existsSync: () => true, readFileSync: () => "release notes" }
      },
    })
  )
  return { creates, updates, errors, outputs, lists }
}

test("fresh release remains a draft until verification publishes it", async () => {
  const result = await run()
  assert.equal(result.creates.length, 1)
  assert.equal(result.creates[0].draft, true)
  assert.equal(result.creates[0].tag_name, tag)
  assert.equal(result.outputs.release_id, "9")
  assert.equal(result.errors.length, 0)
})

test("a prior draft is reused without duplicate creation", async () => {
  const result = await run({ existing: stored() })
  assert.equal(result.creates.length, 0)
  assert.equal(result.updates.length, 0)
  assert.equal(result.outputs.release_id, "7")
})

test("a prior draft with changed notes is updated in place", async () => {
  const result = await run({ existing: { ...stored(), body: "old notes" } })
  assert.equal(result.creates.length, 0)
  assert.equal(result.updates.length, 1)
  assert.equal(result.updates[0].release_id, 7)
  assert.equal(result.updates[0].body, "release notes")
})

test("a formal release cannot be recreated by a tag release", async () => {
  const result = await run({ existing: stored(false) })
  assert.equal(result.errors.length, 1)
  assert.equal(result.creates.length, 0)
  assert.equal(result.updates.length, 0)
})

test("server repair can reuse a formal release", async () => {
  const result = await run({ existing: stored(false), serverOnly: true })
  assert.equal(result.errors.length, 0)
  assert.equal(result.outputs.release_id, "7")
  assert.equal(result.creates.length, 0)
  assert.equal(result.updates.length, 0)
})

test("server repair cannot create a missing release", async () => {
  const result = await run({ serverOnly: true })
  assert.equal(result.errors.length, 1)
  assert.equal(result.creates.length, 0)
})

test("a draft listing failure must not become a duplicate create", async () => {
  await assert.rejects(run({ listError: new Error("API unavailable") }), {
    message: "API unavailable",
  })
})
