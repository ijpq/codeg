// Inventory the static export and migrations embedded in the Rust executable.
import { readFile, writeFile, readdir } from "node:fs/promises"
import { join, relative } from "node:path"
import { createHash } from "node:crypto"
import { execFileSync } from "node:child_process"
const name = "codeg-release-manifest.json"
const root = "out"
const files = {}
async function walk(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) await walk(path)
    else if (entry.isFile() && entry.name !== name)
      files[relative(root, path).replaceAll("\\", "/")] = createHash("sha256")
        .update(await readFile(path))
        .digest("hex")
  }
}
await walk(root)
const source = await readFile("src-tauri/src/db/migration/mod.rs", "utf8")
const migrations = [...source.matchAll(/Box::new\((m\w+)::Migration\)/g)].map(
  (m) => m[1]
)
if (!migrations.length) throw Error("Missing compiled migrations")
for (const migration of migrations)
  await readFile(`src-tauri/src/db/migration/${migration}.rs`)
let commit = process.env.GITHUB_SHA
if (!commit) {
  try {
    commit = execFileSync("git", ["rev-parse", "HEAD"], {
      encoding: "utf8",
    }).trim()
  } catch {
    commit = "source-archive"
  }
}
const { version } = JSON.parse(await readFile("package.json", "utf8"))
await writeFile(
  join(root, name),
  JSON.stringify({ version, commit, migrations, files }, null, 2)
)
console.log(
  `Recorded ${Object.keys(files).length} frontend resources and ${migrations.length} embedded migrations`
)
