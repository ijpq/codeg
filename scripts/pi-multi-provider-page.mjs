// Actual packaged composer -> CodeG -> native Pi ACP/CLI -> two independent
// deterministic provider APIs. This is frontend/protocol evidence, not a claim
// about the user's authenticated relays, latency, billing or model reasoning.
import assert from "node:assert/strict"
import { randomUUID } from "node:crypto"
import { readFile, writeFile } from "node:fs/promises"
import { join } from "node:path"
import { openBusiness } from "./business-agent-page.mjs"
import { businessComposer, reloadBusiness } from "./business-composer-page.mjs"

const pause = (ms) => new Promise((done) => setTimeout(done, ms))
async function until(read, accepts, label) {
  let value
  const deadline = Date.now() + 90_000
  do {
    value = await read()
    if (accepts(value)) return value
    await pause(100)
  } while (Date.now() < deadline)
  assert.fail(`${label}: ${JSON.stringify(value)}`)
}

async function chooseModel(page, opus) {
  const trigger = page
    .getByRole("button", {
      name: /^(Agent settings|智能体设置|智能體設定):/,
    })
    .filter({ visible: true })
    .first()
  await trigger.waitFor({ timeout: 90_000 })
  await trigger.click()
  const panel = page.getByRole("dialog").filter({ visible: true }).last()
  const label = opus ? "Isolated Opus" : "Isolated Pi"
  const search = panel.getByRole("combobox")
  if (await search.count()) await search.first().fill(label)
  const option = panel.getByRole("option", { name: new RegExp(label) }).first()
  await option.waitFor({ timeout: 30_000 })
  await option.click()
  await until(
    () => trigger.getAttribute("aria-label"),
    (value) => value?.includes(label),
    "Selected native model is visible"
  )
  await page.keyboard.press("Escape")
}

async function visibleIdentity(page) {
  return page
    .locator("[data-session-readiness]:visible")
    .first()
    .evaluate((node) => JSON.parse(node.getAttribute("data-session-readiness")))
}

export async function piMultiProviderPage({
  page,
  api,
  f,
  folderId,
  piModel,
  piOpusModel,
  report,
  name,
  output,
  saved = null,
}) {
  if (!piModel || !piOpusModel) return null
  const catalogPath = join(f.env.PI_CODING_AGENT_DIR, "models.json")
  const catalogBefore = await readFile(catalogPath, "utf8")
  const evidence = () => [
    ...piModel.evidence().map((request) => ({ ...request, api: "responses" })),
    ...piOpusModel
      .evidence()
      .map((request) => ({ ...request, api: "messages" })),
  ]
  let cid = saved?.cid ?? null
  let externalId = saved?.externalId ?? null
  let connectionId
  let draftSubmission
  const rounds = []
  try {
    if (saved) {
      await openBusiness(page, api, cid, folderId, "pi")
    } else {
      // Do not create_conversation, connect, or restore via an API first:
      // exercise the real unbound draft and its first business link.
      await page
        .locator(".tab-strip-tail")
        .getByRole("button", {
          name: /^(New Conversation|新建会话|新增會話)$/,
        })
        .first()
        .click()
      await page
        .locator('[data-slot="agent-selector"]:visible')
        .first()
        .waitFor({ timeout: 30_000 })
      const pill = page.locator(
        '[data-slot="agent-pill"][data-agent-type="pi"]:visible'
      )
      if (!(await pill.count())) {
        await page
          .getByRole("button", { name: /More agents|更多智能体|更多智慧體/ })
          .first()
          .click()
        await page
          .locator('[data-slot="agent-option"][data-agent-type="pi"]')
          .click()
      } else await pill.first().click()
    }
    for (let round = 1; round <= (saved ? 2 : 5); round++) {
      const opus = saved ? round === 1 : round === 2 || round === 4
      if (!saved && round === 3) {
        const identity = await visibleIdentity(page)
        await page
          .locator(".tab-strip-tail")
          .getByRole("button", {
            name: /^(New Conversation|新建会话|新增會話)$/,
          })
          .first()
          .click()
        await page
          .locator(`[data-tab-id="${identity.contextKey}"] [role="tab"]`)
          .click()
      }
      if (!saved && round === 4) {
        await reloadBusiness(page, api, {
          conversationId: cid,
          folderId,
          agent: "pi",
        })
      }
      await chooseModel(page, opus)
      if (cid != null) {
        const before = await visibleIdentity(page)
        assert.equal(before.selectedAgent, "pi")
        assert.equal(before.conversationId, cid)
      } else {
        assert.equal(
          await page.locator("[data-business-agent-toolbar]:visible").count(),
          0,
          "First input is in an unbound draft"
        )
      }
      const marker = `PI_FRONTEND_${name.toUpperCase().replace(/[^A-Z0-9]/g, "_")}_${round}_${randomUUID()}`
      const editor =
        cid != null
          ? await businessComposer(page, { conversationId: cid, agent: "pi" })
          : page
              .locator('[contenteditable="true"]')
              .filter({ visible: true })
              .first()
      await editor.fill(marker)
      if (cid == null) {
        const send = page
          .getByRole("button", { name: /^(Send|发送|傳送|發送)$/ })
          .filter({ visible: true })
          .first()
        draftSubmission = {
          sendDisabledWhenFilled: await send.isDisabled(),
          draftStillContainsInput: (await editor.innerText()).includes(marker),
          selectedAgent: "pi",
        }
        // An unbound draft deliberately disables submission while its native
        // selector/connection initializes. Enter during that disabled state is
        // not an accepted message. Click the real affordance exactly once;
        // Playwright waits for it to be usable, with no API connect/restore or
        // pre-created business. Persisted recovery still uses Enter below and
        // is separately tested while its native session is explicitly blocked.
        await send.click({ timeout: 90_000 })
        draftSubmission.clickedSendOnce = true
      } else {
        await editor.press("Enter")
      }
      if (cid == null) {
        cid = (
          await until(
            () => api("list_all_conversations"),
            (rows) => rows.some((row) => row.title?.includes(marker)),
            "First frontend send created its business conversation"
          )
        ).find((row) => row.title?.includes(marker)).id
      }
      const history = await until(
        () =>
          api("get_folder_conversation", {
            conversationId: cid,
            userTurnLimit: 25,
          }),
        (value) => {
          const index = value.turns.findIndex(
            (turn) =>
              turn.role === "user" &&
              JSON.stringify(turn.blocks).includes(marker)
          )
          return (
            index >= 0 &&
            value.turns
              .slice(index + 1)
              .some(
                (turn) =>
                  turn.role === "assistant" &&
                  JSON.stringify(turn.blocks).includes("BUSINESS_PI_COMPLETED")
              )
          )
        },
        "Frontend input receives a durable native Pi reply"
      )
      const live = await until(
        () =>
          api("acp_find_connection_for_conversation", {
            conversationId: cid,
            agentType: "pi",
          }),
        (value) => value?.connection_id,
        "Business connection remains bound"
      )
      await until(
        () =>
          api("acp_get_session_snapshot", { connectionId: live.connection_id }),
        (snapshot) =>
          snapshot?.status === "connected" &&
          !snapshot.turn_in_flight &&
          snapshot.selectors_ready,
        "Native round completed before the next action"
      )
      if (connectionId)
        assert.equal(
          live.connection_id,
          connectionId,
          "Model selection and identity updates must not replace the connection"
        )
      connectionId = live.connection_id
      if (externalId)
        assert.equal(
          history.summary.external_id,
          externalId,
          "Model changes preserve the native session"
        )
      externalId = history.summary.external_id
      await pause(250)
      const sent = evidence().filter(
        (request) => request.frontendMarker === marker
      )
      assert.equal(
        sent.length,
        1,
        "Exactly one actual provider request for this frontend input"
      )
      assert.equal(sent[0].api, opus ? "messages" : "responses")
      assert.equal(sent[0].model, opus ? "claude/claude-opus-5-5" : "gpt-5.4")
      assert.equal(
        history.turns.filter(
          (turn) =>
            turn.role === "user" && JSON.stringify(turn.blocks).includes(marker)
        ).length,
        1
      )
      rounds.push({
        round,
        opus,
        marker,
        connectionId,
        externalId,
        actualProviderRequests: sent.length,
        identity: await visibleIdentity(page),
      })
    }
    assert.equal(
      await readFile(catalogPath, "utf8"),
      catalogBefore,
      "Both independent Provider configurations are retained"
    )
    await page.screenshot({
      path: join(output, `${name}-pi-multi-provider.png`),
    })
    const result = { cid, externalId, folderId, draftSubmission, rounds }
    await writeFile(
      join(output, `${name}-pi-multi-provider.json`),
      JSON.stringify(result, null, 2)
    )
    report.checks.push({
      name: `${name}-pi-multi-provider-composer`,
      passed: true,
      actualComposer: true,
      freshDraft: !saved,
      restart: !!saved,
      rounds: rounds.length,
      independentApis: ["openai-responses", "anthropic-messages"],
      sameNativeSession: true,
      providerConfigsRetained: true,
      protocol:
        "Packaged native Pi/ACP; deterministic independent model endpoints",
    })
    return result
  } catch (error) {
    await page
      .screenshot({
        path: join(output, `${name}-pi-multi-provider-failure.png`),
      })
      .catch(() => {})
    await writeFile(
      join(output, `${name}-pi-multi-provider-failure.json`),
      JSON.stringify(
        {
          error: String(error.stack ?? error),
          cid,
          rounds,
          draftSubmission,
          identity: await visibleIdentity(page).catch(() => null),
          providerRequests: evidence()
            .filter((request) => request.frontendMarker)
            .map(({ frontendMarker, model, api }) => ({
              frontendMarker,
              model,
              api,
            })),
        },
        null,
        2
      )
    )
    throw error
  }
}
