// Actual converter + Pi MCP + CodeG settlement, only disposable test data.
import assert from "node:assert/strict"
import { execFile } from "node:child_process"
import { promisify } from "node:util"
import { createHash, randomUUID } from "node:crypto"
import { readFile, writeFile } from "node:fs/promises"
import { join } from "node:path"
const execute = promisify(execFile)
const delay = (ms) => new Promise((done) => setTimeout(done, ms))
async function until(read, accepts, label) {
  for (let i = 0; i < 900; i++) {
    const value = await read()
    if (accepts(value)) return value
    await delay(100)
  }
  throw Error(label)
}
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex")

export async function officeDeliveryChecks({
  api,
  f,
  cid,
  page,
  piModel,
  submit,
  finish,
  ready,
  snapshot,
  report,
  name,
}) {
  const converter = await api("officecli_detect")
  assert.ok(
    converter.installed && !converter.runtimeError,
    "The isolated real Office converter must run"
  )
  const office = (...args) =>
    execute(converter.path, args, { env: f.env, timeout: 30000 })
  const makeDoc = async (file, text) => {
    const path = join(f.cwd, file)
    await office("create", path, "--force")
    await office(
      "add",
      path,
      "/body",
      "--type",
      "paragraph",
      "--prop",
      `text=${text}`
    )
    await office("close", path)
    return readFile(path)
  }
  await makeDoc("office-process.docx", "OFFICE_PROCESS_DO_NOT_AUTO_OPEN")
  const v1 = await makeDoc("office-review.docx", "OFFICE_REVIEW_V1")
  const visible = () => page?.locator("[data-office-review-run]:visible")
  if (page)
    assert.equal(
      await visible().count(),
      0,
      "Workspace writes alone cannot open a preview"
    )
  const begin = async (path, clear = false) => {
    piModel.holdOffice(true)
    const receipt = await submit({
      blocks: [
        {
          type: "text",
          text: `BUSINESS_PI_USER PI_SCENARIO_${clear ? "officeclear" : "office"}_${randomUUID().replaceAll("-", "")} OFFICE_PATH=${path}`,
        },
      ],
    })
    await until(
      () =>
        api("get_folder_conversation", {
          conversationId: cid,
          userTurnLimit: 25,
        }),
      (detail) => {
        const set = detail.deliverable_runs?.find(
          (r) => r.client_message_id === receipt.clientMessageId
        )
        const run = detail.artifact_runs?.find(
          (r) => r.client_message_id === receipt.clientMessageId
        )
        // The public card list intentionally omits empty sets. The durable
        // declaration status distinguishes an intentional clear from a read
        // that merely raced the tool, without inventing an empty card.
        if (clear)
          return Boolean(
            run?.declaration_status === "success_empty" &&
            (!set || set.deliverables.length === 0)
          )
        return Boolean(set && set.deliverables.some((d) => d.path === path))
      },
      "Final Office declaration persisted during the running task"
    )
    assert.equal((await snapshot()).status, "prompting")
    return receipt
  }
  const complete = async (receipt) => {
    piModel.holdOffice(false)
    const done = await finish(receipt.id)
    assert.equal(done.status, "completed")
    await ready()
    return until(
      () =>
        api("get_office_delivery_preview", {
          conversationId: cid,
          turnRunId: done.taskId,
        }),
      Boolean,
      "Office settlement review"
    )
  }
  const show = async (review, expectedText) => {
    if (!page) return
    const header = page.locator(
      `[data-office-review-run="${review.turn_run_id}"]:visible`
    )
    await header.waitFor({ timeout: 30000 })
    assert.equal(
      await header.getAttribute("data-office-review-sha"),
      review.sha256
    )
    const preview = header.locator("..").frameLocator("iframe")
    await until(
      () =>
        preview
          .locator("body")
          .innerText()
          .catch(() => ""),
      (text) => text.includes(expectedText),
      "Actual Office document visible in the app sidebar"
    )
  }
  const verifyReviewDownload = async (review) => {
    const destination = join(
      f.root,
      `review-download-${review.turn_run_id}.docx`
    )
    if (name === "setup") {
      await api("save_office_delivery_preview", {
        conversationId: cid,
        turnRunId: review.turn_run_id,
        destination,
      })
    } else if (page) {
      const download = page.waitForEvent("download", { timeout: 30000 })
      await page
        .locator(`[data-office-review-run="${review.turn_run_id}"]:visible`)
        .getByRole("button", {
          name: /Download this review copy|下载此审阅副本/,
        })
        .click()
      await (await download).saveAs(destination)
    } else return
    assert.equal(
      hash(await readFile(destination)),
      review.sha256,
      "Download must return the exact reviewed bytes"
    )
  }
  try {
    const firstTask = await begin("office-review.docx")
    if (page)
      assert.equal(
        await visible().count(),
        0,
        "In-progress MCP declarations cannot open process drafts"
      )
    const first = await complete(firstTask)
    assert.equal(first.status, "ready")
    assert.equal(first.sha256, hash(v1))
    await show(first, "OFFICE_REVIEW_V1")
    const v2 = await makeDoc("office-review.docx", "OFFICE_REVIEW_V2")
    const secondTask = await begin("office-review.docx")
    assert.equal(
      hash(await readFile(first.snapshot_path)),
      hash(v1),
      "The review copy survives same-path draft edits"
    )
    if (page) {
      assert.equal(
        await visible().getAttribute("data-office-review-run"),
        first.turn_run_id
      )
      await show(first, "OFFICE_REVIEW_V1")
    }
    await verifyReviewDownload(first)
    const second = await complete(secondTask)
    assert.notEqual(second.snapshot_path, first.snapshot_path)
    assert.equal(second.sha256, hash(v2))
    await show(second, "OFFICE_REVIEW_V2")
    const renamedBytes = await makeDoc(
      "office-review-v3.docx",
      "OFFICE_REVIEW_V3"
    )
    const third = await complete(await begin("office-review-v3.docx"))
    assert.equal(third.sha256, hash(renamedBytes))
    assert.ok(third.original_path.endsWith("office-review-v3.docx"))
    await show(third, "OFFICE_REVIEW_V3")
    const withdrawn = await complete(await begin("office-review-v3.docx", true))
    assert.equal(withdrawn.status, "none")
    if (page)
      assert.equal(
        await visible().getAttribute("data-office-review-run"),
        third.turn_run_id,
        "A withdrawn declaration never replaces the last delivered review"
      )

    // The same existing renderer handles all three native OpenXML formats.
    for (const extension of ["xlsx", "pptx"]) {
      const path = join(f.cwd, `office-format.${extension}`)
      await office("create", path, "--force")
      const marker = `OFFICE_FORMAT_${extension.toUpperCase()}`
      if (extension === "xlsx")
        await office("set", path, "/Sheet1/A1", "--prop", `value=${marker}`)
      else
        await office(
          "add",
          path,
          "/",
          "--type",
          "slide",
          "--prop",
          `title=${marker}`
        )
      await office("close", path)
      const started = await api("start_office_watch", {
        rootPath: f.cwd,
        path: `office-format.${extension}`,
      })
      assert.ok(started.port && started.cap)
      const rendered = await fetch(`http://127.0.0.1:${started.port}/`).then(
        (response) => response.text()
      )
      assert.ok(
        rendered.includes(marker),
        `${extension} review must contain the actual delivered content`
      )
      await api("stop_office_watch", {
        rootPath: f.cwd,
        path: `office-format.${extension}`,
        cap: started.cap,
        hold: started.hold,
      })
    }
    if (page) {
      // A page refresh restores cards but does not replay every completion.
      await page.reload()
      await page
        .locator("[data-business-agent-control]:visible")
        .waitFor({ timeout: 90000 })
      assert.equal(await visible().count(), 0)
      const card = page
        .locator(`[data-deliverable-run="${third.turn_run_id}"]`)
        .first()
      await card.waitFor({ timeout: 30000 })
      await card
        .getByRole("button", { name: /View in CodeG|在 CodeG 中查看/ })
        .click()
      await show(third, "OFFICE_REVIEW_V3")
      await page.screenshot({
        path: join(f.root, "office-delivered-review.png"),
      })
      const tab = page
        .locator("[data-file-tab-id]")
        .filter({ hasText: "office-review-v3.docx" })
        .first()
      await tab.locator("button").last().click()
      await writeFile(
        join(f.cwd, "office-process.docx"),
        "another process write"
      )
      await delay(250)
      assert.equal(
        await visible().count(),
        0,
        "Closing review cannot be undone by workspace events"
      )
    }
    report.checks.push({
      name: `${name}-office-delivery-preview`,
      passed: true,
      realConverter: converter.version,
      completionAndSettlementOnly: true,
      finalReplaceAndClear: true,
      immutableReview: true,
      samePathUpdated: true,
      renamedPathUpdated: true,
      threeFormats: true,
      manualAfterRefresh: Boolean(page),
      closedNotReopenedByWrites: Boolean(page),
      originalBytesPreserved: true,
      reviewDownloadVerified: Boolean(page),
    })
  } finally {
    piModel.holdOffice(false)
  }
}
