// Runs in a disposable CI browser; never discovers a running CodeG instance.
import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { createServer } from "node:http"
import { mkdir, readFile, writeFile } from "node:fs/promises"
import { extname, join, resolve } from "node:path"
import { build } from "esbuild"
import postcss from "postcss"
import tailwind from "@tailwindcss/postcss"
import { chromium } from "playwright-core"

const output = resolve(
  process.env.CODEG_BROWSER_EVIDENCE ?? "artifacts/browser-scroll",
  "file-tabs"
)
const baselineFlag = process.argv.indexOf("--baseline-ref")
const baseline = baselineFlag < 0 ? null : process.argv[baselineFlag + 1]
if (baselineFlag >= 0) assert.match(baseline ?? "", /^[a-f0-9]{40}$/)
const services = resolve("tests/browser/file-tabs.tsx")
const variants = baseline ? ["baseline", "current"] : ["current"]
for (const variant of variants) {
  await mkdir(join(output, variant), { recursive: true })
  await build({
    entryPoints: [services],
    bundle: true,
    outdir: join(output, variant),
    platform: "browser",
    format: "iife",
    jsx: "automatic",
    define: { "process.env.NODE_ENV": '"production"' },
    loader: { ".woff2": "file", ".woff": "file", ".ttf": "file" },
    plugins: [
      {
        name: "isolated-host-services",
        setup(b) {
          b.onResolve(
            {
              filter:
                /^@\/(contexts\/workspace-context|lib\/(transport|platform|browser\/(browser-api|use-browser-capabilities|browser-tab-store))|components\/browser\/browser-agent-access)$/,
            },
            () => ({ path: services })
          )
          b.onLoad({ filter: /globals\.css$/ }, async ({ path }) => ({
            contents: (
              await postcss([tailwind()]).process(
                await readFile(path, "utf8"),
                { from: path }
              )
            ).css,
            loader: "css",
            resolveDir: resolve("src/app"),
          }))
          if (variant === "baseline")
            b.onLoad({ filter: /file-workspace-tab-bar\.tsx$/ }, () => ({
              contents: execFileSync(
                "git",
                [
                  "show",
                  `${baseline}:src/components/files/file-workspace-tab-bar.tsx`,
                ],
                { encoding: "utf8" }
              ),
              loader: "tsx",
              resolveDir: resolve("src/components/files"),
            }))
        },
      },
    ],
  })
}
if (process.argv.includes("--build-only")) process.exit(0)

const server = createServer(async (request, response) => {
  try {
    const path = new URL(request.url, "http://127.0.0.1").pathname
    const parts = path.split("/").filter(Boolean)
    if (!variants.includes(parts[0]) || parts.length > 2) throw Error("path")
    if (parts.length === 1) {
      response.setHeader("Content-Type", "text/html")
      response.end(
        `<html><head><link rel="stylesheet" href="/${parts[0]}/file-tabs.css"></head><body style="margin:20px"><div id="root"></div><script src="/${parts[0]}/file-tabs.js"></script></body></html>`
      )
      return
    }
    if (!/^[\w.-]+$/.test(parts[1])) throw Error("path")
    const type = {
      ".js": "text/javascript",
      ".css": "text/css",
      ".woff2": "font/woff2",
    }[extname(parts[1])]
    if (type) response.setHeader("Content-Type", type)
    response.end(await readFile(join(output, ...parts)))
  } catch {
    response.writeHead(404)
    response.end()
  }
})
await new Promise((done) => server.listen(0, "127.0.0.1", done))
const browser = await chromium.launch({ headless: true })
const results = []
const errors = []
let passed = false
try {
  const page = await browser.newPage({ viewport: { width: 1000, height: 300 } })
  page.on("pageerror", (error) => errors.push(String(error)))
  for (const variant of variants) {
    await page.goto(`http://127.0.0.1:${server.address().port}/${variant}/`)
    await page.waitForFunction(() => Boolean(window.fileTabsFixture))
    for (const width of [84, 140, 320, 640]) {
      for (const count of [1, 2, 6, 12]) {
        for (const active of [...new Set([0, count - 1])]) {
          await page.evaluate(
            ({ width, count, active }) =>
              window.fileTabsFixture.configure(width, count, active),
            { width, count, active }
          )
          const tab = page.locator(
            `[data-file-tab-id="file:/fixture/office-review-${active}.docx"]`
          )
          // The baseline bug can collapse the tab to zero width. It must be
          // attached for measurement; current hit targets still have to pass
          // the browser's real visibility checks when clicked below.
          await tab.waitFor({ state: "attached" })
          const close = tab.getByRole("button", { name: "Close file tab" })
          // Reorder animates between the preceding fixture's widths. Wait for
          // the same stable, unobstructed target a real click requires before
          // measuring; a fixed delay can sample an intermediate scale transform.
          let actionReady = false
          let actionError = null
          try {
            await close.click({
              trial: true,
              timeout: variant === "baseline" ? 1200 : 5000,
            })
            actionReady = true
          } catch (error) {
            actionError = String(error).split("\n")[0]
          }
          const geometry = await close.evaluate((button) => {
            const rect = button.getBoundingClientRect()
            const strip = button
              .closest('[role="tablist"]')
              .getBoundingClientRect()
            const item = button
              .closest("[data-file-tab-id]")
              .getBoundingClientRect()
            const x = rect.x + rect.width / 2,
              y = rect.y + rect.height / 2
            return {
              tabWidth: item.width,
              minimumWidth: getComputedStyle(
                button.closest("[data-file-tab-id]")
              ).minWidth,
              closeLeft: rect.left,
              closeRight: rect.right,
              stripLeft: strip.left,
              stripRight: strip.right,
              fullyContained:
                rect.left >= strip.left - 0.5 &&
                rect.right <= strip.right + 0.5,
              hit: button.contains(document.elementFromPoint(x, y)),
            }
          })
          if (width === 84 && count === 2 && active === 1)
            await page.screenshot({
              path: join(output, `${variant}-narrow.png`),
            })
          let clicked = false
          if (actionReady && geometry.hit) {
            const before = await page.evaluate(
              () => window.fileTabsFixture.closed.length
            )
            // Keep Playwright's actual visibility and hit-target checks enabled.
            await close.click({ timeout: 2000 })
            clicked = await page.evaluate(
              ({ before, active }) =>
                window.fileTabsFixture.closed.length === before + 1 &&
                window.fileTabsFixture.closed.at(-1) ===
                  `file:/fixture/office-review-${active}.docx`,
              { before, active }
            )
          }
          results.push({
            variant,
            width,
            count,
            active,
            ...geometry,
            actionReady,
            actionError,
            clicked,
          })
        }
      }
    }
  }
  assert.deepEqual(errors, [])
  const failed = results.filter(
    (row) => row.variant === "current" && (!row.clicked || !row.fullyContained)
  )
  assert.equal(failed.length, 0, JSON.stringify(failed))
  if (baseline) {
    assert.ok(
      results.some(
        (row) =>
          row.variant === "baseline" &&
          row.width === 84 &&
          row.count === 2 &&
          row.active === 1 &&
          !row.clicked
      ),
      "Must reproduce the packaged Office close-button obstruction"
    )
    assert.ok(
      results.some(
        (row) =>
          row.variant === "baseline" &&
          row.width === 640 &&
          row.count === 1 &&
          row.clicked
      ),
      "The baseline harness must also demonstrate a usable wide tab"
    )
  }
  passed = true
} finally {
  await writeFile(
    join(output, "verification.json"),
    JSON.stringify(
      {
        passed,
        baseline,
        results,
        errors,
      },
      null,
      2
    )
  )
  await browser.close()
  server.close()
}
