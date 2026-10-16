// Run in an isolated CI browser, or against an explicitly supplied WebView2 CDP
// endpoint. Never locates or attaches to a user's running CodeG instance.
import { build } from "esbuild"
import postcss from "postcss"
import tailwind from "@tailwindcss/postcss"
import { chromium } from "playwright-core"
import { createServer } from "node:http"
import { readFile, mkdir, writeFile } from "node:fs/promises"
import { resolve, extname, join } from "node:path"
import { execFileSync } from "node:child_process"
import assert from "node:assert/strict"
const output = resolve(
  process.env.CODEG_BROWSER_EVIDENCE ?? "artifacts/browser-scroll"
)
await mkdir(output, { recursive: true })
const baseline = process.argv.includes("--baseline")
await build({
  entryPoints: ["tests/browser/transcript.tsx"],
  bundle: true,
  outdir: join(output, "site"),
  format: "iife",
  platform: "browser",
  sourcemap: true,
  jsx: "automatic",
  define: { "process.env.NODE_ENV": '"production"' },
  loader: { ".woff2": "file", ".woff": "file", ".ttf": "file" },
  plugins: [
    {
      name: "production-css-and-baseline",
      setup(b) {
        b.onLoad({ filter: /globals\.css$/ }, async ({ path }) => ({
          contents: (
            await postcss([tailwind()]).process(await readFile(path, "utf8"), {
              from: path,
            })
          ).css,
          loader: "css",
          resolveDir: resolve("src/app"),
        }))
        if (baseline)
          b.onLoad(
            { filter: /message-bottom-follow-guard\.tsx$/ },
            ({ path }) => ({
              contents: execFileSync(
                "git",
                [
                  "show",
                  "b8cae024:src/components/message/message-bottom-follow-guard.tsx",
                ],
                { encoding: "utf8" }
              ),
              loader: "tsx",
              resolveDir: resolve(path, ".."),
            })
          )
      },
    },
  ],
})
if (process.argv.includes("--build-only")) process.exit(0)
const server = createServer(async (req, res) => {
  try {
    if (req.url === "/" || req.url?.startsWith("/?")) {
      res.setHeader("Content-Type", "text/html")
      res.end(
        '<html><head><link rel="stylesheet" href="/transcript.css"></head><body style="margin:0"><div id="root"></div><script src="/transcript.js"></script></body></html>'
      )
      return
    }
    if (req.url === "/late-image.svg") {
      await new Promise((done) => setTimeout(done, 350))
      res.setHeader("Content-Type", "image/svg+xml")
      res.end(
        '<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="830"><rect width="1200" height="830" fill="#b5cddd"/></svg>'
      )
      return
    }
    const path = join(output, "site", req.url.split("?")[0])
    if (!path.startsWith(join(output, "site"))) throw Error("path")
    const type = {
      ".js": "text/javascript",
      ".css": "text/css",
      ".woff2": "font/woff2",
    }[extname(path)]
    if (type) res.setHeader("Content-Type", type)
    res.end(await readFile(path))
  } catch {
    res.writeHead(404)
    res.end()
  }
})
await new Promise((done) => server.listen(0, "127.0.0.1", done))
const port = server.address().port
const remote = process.env.CODEG_TEST_WEBVIEW_CDP
const browser = remote
  ? await chromium.connectOverCDP(remote)
  : await chromium.launch({
      channel: process.env.CODEG_TEST_BROWSER_CHANNEL || undefined,
      headless: true,
    })
const results = []
const failures = []
let testPage
try {
  const context = remote
    ? browser.contexts()[0]
    : await browser.newContext({
        viewport: { width: 1000, height: 800 },
        deviceScaleFactor: 1.25,
      })
  const page = remote ? context.pages()[0] : await context.newPage()
  testPage = page
  const errors = []
  page.on("pageerror", (error) => errors.push(String(error)))
  await page.goto(`http://127.0.0.1:${port}`)
  await page.waitForFunction(() => Boolean(window.scrollFixture))
  const change = async (next) => {
    await page.evaluate((next) => window.scrollFixture.change(next), next)
    await page.waitForTimeout(500)
  }
  const geometry = async () =>
    page.evaluate(() => {
      const v = document.querySelector(".chat-scroll-port"),
        line = document.querySelector("[data-final-line]"),
        sentinel = document.querySelector("[data-message-end-sentinel]")
      const vr = v.getBoundingClientRect(),
        lr = line?.getBoundingClientRect()
      return {
        debug: window.scrollFixture.debug?.(),
        frames: window.scrollFixture.frames,
        following: v.dataset.bottomFollowing,
        top: v.scrollTop,
        height: v.scrollHeight,
        viewport: v.clientHeight,
        remaining: v.scrollHeight - v.clientHeight - v.scrollTop,
        lineBottom: lr?.bottom,
        viewportBottom: vr.bottom,
        sentinel: sentinel.getBoundingClientRect().bottom,
        correction: sentinel.style.marginTop,
        rendered: document.querySelectorAll("[data-message-thread-row]").length,
      }
    })
  const stable = async (name) => {
    await page.waitForTimeout(700)
    const samples = []
    for (let i = 0; i < 12; i++) {
      samples.push(await geometry())
      await page.waitForTimeout(60)
    }
    const spread =
      Math.max(...samples.map((x) => x.top)) -
      Math.min(...samples.map((x) => x.top))
    const last = samples.at(-1)
    results.push({ name, spread, samples })
    try {
      assert.ok(
        last.frames - samples[0].frames < 8,
        `${name}: animation frames never settled`
      )
      assert.ok(spread <= 1, `${name}: unstable scroll ${spread}px`)
      assert.ok(last.remaining <= 2, `${name}: tail gap ${last.remaining}px`)
      assert.ok(
        last.lineBottom <= last.viewportBottom + 1,
        `${name}: final line clipped`
      )
      assert.ok(
        last.rendered < 80,
        `${name}: virtualization lost (${last.rendered})`
      )
    } catch (error) {
      failures.push(String(error))
    }
  }
  for (const scale of [1, 1.25, 1.5, 1.75, 2]) {
    await change({ scale, lines: 50, smooth: false })
    await stable(`long-complete-${scale}`)
    for (let lines = 51; lines <= 65; lines++) {
      await page.evaluate(
        (lines) => window.scrollFixture.change({ lines }),
        lines
      )
      await page.waitForTimeout(18)
    }
    await stable(`stream-end-${scale}`)
    await change({ image: true, artifact: true, dock: 280 })
    await stable(`late-load-${scale}`)
    await change({ dock: 80 })
    await stable(`dock-shrink-${scale}`)
    await change({ smooth: true, lines: 75 })
    await stable(`smooth-finish-${scale}`)
    // A trusted wheel event must escape even while rows/viewport resize.
    await page.locator(".chat-scroll-port").hover()
    await page.mouse.wheel(0, -600)
    await page.waitForTimeout(200)
    const before = await geometry()
    await change({ lines: 85, dock: 200 })
    const after = await geometry()
    results.push({ name: `reader-escape-${scale}`, before, after })
    try {
      assert.ok(after.remaining > 100, "reader was pulled back to the bottom")
      assert.ok(
        Math.abs(after.top - before.top) < 3,
        "reader's position was changed"
      )
    } catch (error) {
      failures.push(String(error))
    }
    // Re-arm using the same explicit action as the scroll-to-bottom button.
    await page.evaluate(() => {
      const v = document.querySelector(".chat-scroll-port")
      v.scrollTop = v.scrollHeight
    })
    await page.waitForTimeout(200)
    await change({
      image: false,
      artifact: false,
      lines: 2,
      dock: 100,
      smooth: false,
    })
    await stable(`settled-${scale}`)
  }
  assert.deepEqual(errors, [])
  await page.screenshot({ path: join(output, "transcript.png") })
  if (failures.length) throw new Error(failures.join("\n"))
  await page.close()
} finally {
  await writeFile(
    join(output, "scroll-results.json"),
    JSON.stringify(
      {
        baseline,
        transitions: await testPage
          ?.evaluate(() => window.scrollTransitions)
          .catch(() => null),
        failures,
        results,
        writes: await testPage
          ?.evaluate(() => window.scrollFixture?.writes)
          .catch(() => null),
      },
      null,
      2
    )
  )
  await browser.close()
  server.close()
}
console.log(JSON.stringify({ passed: true, baseline, cases: results.length }))
