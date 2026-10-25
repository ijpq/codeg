// Run against disposable packaged Server/Tauri transports and the real Pi CLI.
import assert from "node:assert/strict"
import { randomUUID } from "node:crypto"
import { readFile } from "node:fs/promises"
import { join } from "node:path"
import { officeDeliveryChecks } from "./office-delivery-checks.mjs"
import { openBusiness } from "./business-agent-page.mjs"

const delay = (ms) => new Promise((done) => setTimeout(done, ms))
async function until(read, accepts, label) {
  let value
  for (let i = 0; i < 900; i++) {
    value = await read()
    if (accepts(value)) return value
    await delay(100)
  }
  throw Error(`${label}: ${JSON.stringify(value)}`)
}
const terminal = (r) =>
  ["completed", "failed", "cancelled", "uncertain"].includes(r.status)

export async function piCapabilityChecks({
  api,
  f,
  folderId,
  piModel,
  report,
  name,
  download,
  page,
}) {
  if (!piModel) return
  const defaults = await api("get_delegation_settings")
  await api("set_delegation_settings", {
    settings: { ...defaults, enabled: true, depth_limit: 2 },
  })
  const cid = await api("create_conversation", {
    folderId,
    agentType: "pi",
    title: "Isolated native Pi capabilities",
  })
  const connectionId = await api("acp_connect", {
    agentType: "pi",
    workingDir: f.cwd,
  })
  const snapshot = () => api("acp_get_session_snapshot", { connectionId })
  await until(
    snapshot,
    (s) => s?.selectors_ready && s.external_id,
    "Pi ready verifies identity"
  )
  const set = (configId, valueId) =>
    api("acp_set_config_option", { connectionId, configId, valueId })
  const ready = () =>
    until(snapshot, (s) => s.status === "connected", "Pi task settled")
  const start = async (text) => {
    await ready()
    await api("acp_prompt", {
      connectionId,
      conversationId: cid,
      folderId,
      clientMessageId: randomUUID(),
      blocks: [{ type: "text", text }],
    })
  }
  const run = async (text, expected) => {
    await start(text)
    await until(
      async () => {
        try {
          return await api("get_folder_conversation", {
            conversationId: cid,
            userTurnLimit: 25,
          })
        } catch (error) {
          if (/not.found/i.test(String(error))) return { turns: [] }
          throw error
        }
      },
      (detail) => {
        const index = detail.turns.findIndex(
          (t) => t.role === "user" && JSON.stringify(t.blocks).includes(text)
        )
        return (
          index >= 0 &&
          detail.turns
            .slice(index + 1)
            .some(
              (t) =>
                t.role === "assistant" &&
                JSON.stringify(t.blocks).includes(expected)
            )
        )
      },
      expected
    )
    await ready()
  }
  const submit = (request) =>
    api("submit_business_message", {
      request: {
        conversationId: cid,
        clientMessageId: randomUUID(),
        blocks: [{ type: "text", text: "BUSINESS_PI_USER external queue" }],
        ...request,
      },
    })
  const receipt = (id) =>
    api("get_business_message", { conversationId: cid, messageId: id })
  const finish = (id) =>
    until(() => receipt(id), terminal, "External message terminal receipt")
  try {
    await set("model", "fixture/gpt-5.4")
    await run("BUSINESS_PI_USER first native request", "BUSINESS_PI_COMPLETED")
    assert.equal(piModel.evidence().at(-1).tier, "default")
    await set("service_tier", "on")
    await run("BUSINESS_PI_USER priority request", "BUSINESS_PI_COMPLETED")
    assert.equal(piModel.evidence().at(-1).tier, "priority")
    assert.equal(piModel.evidence().at(-1).effort, "medium")
    await assert.rejects(
      submit({ requiredAgent: "codex" }),
      /REQUIRED_AGENT_MISMATCH/
    )
    await assert.rejects(
      submit({ mode: "follow_up" }),
      /DELIVERY_MODE_UNSUPPORTED/
    )

    const stable = {
      clientMessageId: randomUUID(),
      requiredAgent: "pi",
      metadata: { source: "synthetic-external-program" },
    }
    const [one, duplicate] = await Promise.all([submit(stable), submit(stable)])
    assert.equal(one.id, duplicate.id)
    assert.equal(one.targetSettings.config.service_tier, "on")
    assert.equal((await finish(one.id)).status, "completed")
    assert.equal((await submit(stable)).id, one.id)
    await assert.rejects(
      submit({ ...stable, blocks: [{ type: "text", text: "changed body" }] }),
      /IDEMPOTENCY_KEY_REUSED/
    )
    const beforeImage = piModel.evidence().length
    const withImage = await submit({
      blocks: [
        { type: "text", text: "BUSINESS_PI_USER external image" },
        {
          type: "image",
          mime_type: "image/png",
          data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNoaGgAAAMEAYFL09IQAAAAAElFTkSuQmCC",
        },
      ],
    })
    assert.equal((await finish(withImage.id)).status, "completed")
    assert.ok(
      piModel
        .evidence()
        .slice(beforeImage)
        .some((r) => r.image),
      "An external attachment must reach the actual Pi provider request"
    )

    // A queued ordinary message must not block guidance for the active task.
    piModel.delay(1800)
    await start("BUSINESS_PI_USER guidance owner")
    await until(
      snapshot,
      (s) => s.status === "prompting" && s.supports_steer,
      "Pi running steer capability"
    )
    const queued = await submit({})
    const guided = await submit({
      mode: "steer",
      blocks: [{ type: "text", text: "PI_NATIVE_GUIDANCE" }],
    })
    await assert.rejects(
      set("service_tier", "off"),
      /PI_SETTINGS_BUSY|in.progress|busy/i
    )
    assert.equal((await finish(guided.id)).status, "completed")
    assert.equal((await finish(queued.id)).status, "completed")
    assert.equal(queued.targetSettings.config.service_tier, "on")
    assert.equal(piModel.evidence().at(-1).tier, "priority")
    assert.equal(piModel.evidence().at(-1).effort, "medium")
    assert.ok(
      piModel.evidence().some((r) => r.steer),
      "Guidance must reach a later actual Pi model request"
    )
    piModel.delay(50)

    const marker = randomUUID().replaceAll("-", "")
    if (page) await openBusiness(page, api, cid, folderId, "pi")
    await run(
      `BUSINESS_PI_USER PI_SCENARIO_publish_${marker}`,
      "PI_PUBLISH_COMPLETED"
    )
    const items = await until(
      () => api("list_conversation_deliverables", { conversationId: cid }),
      (rs) => rs.some((r) => r.title === "Pi verified report"),
      "Pi MCP registered deliverable"
    )
    const item = items.find((r) => r.title === "Pi verified report")
    assert.match(
      await readFile(join(f.cwd, "pi-verified-report.md"), "utf8"),
      /PI_VERIFIED_CONTENT/
    )
    await download(cid, item.id)
    const declaredDetail = await api("get_folder_conversation", {
      conversationId: cid,
      userTurnLimit: 25,
    })
    const publishedRun = declaredDetail.deliverable_runs.find((r) =>
      r.deliverables.some((d) => d.id === item.id)
    )
    assert.ok(
      publishedRun?.user_turn_id,
      "Published file must have a durable producing user turn"
    )
    assert.ok(
      JSON.stringify(
        declaredDetail.turns.find((t) => t.id === publishedRun.user_turn_id)
          ?.blocks
      ).includes(`PI_SCENARIO_publish_${marker}`)
    )
    const refresh = await api("list_conversation_output_window", {
      conversationId: cid,
      turnRefs: declaredDetail.output_turn_refs,
    })
    assert.equal(
      refresh.deliverable_runs.find(
        (r) => r.turn_run_id === publishedRun.turn_run_id
      )?.user_turn_id,
      publishedRun.user_turn_id
    )
    const card = () =>
      page.locator(`[data-deliverable-run="${publishedRun.turn_run_id}"]`)
    if (page) {
      await card().waitFor({ timeout: 30000 })
      assert.equal(
        await card().count(),
        1,
        "Live MCP declaration must appear without re-entering the conversation"
      )
      await page.reload()
      await card().waitFor({ timeout: 30000 })
    }
    // A second external task edits the same physical path. The aggregate id
    // stays stable while each inline card keeps its own run association.
    const secondPublish = await submit({
      blocks: [
        {
          type: "text",
          text: `BUSINESS_PI_USER PI_SCENARIO_publish_${randomUUID().replaceAll("-", "")}`,
        },
      ],
    })
    assert.equal((await finish(secondPublish.id)).status, "completed")
    const versions = await api("list_conversation_deliverable_history", {
      conversationId: cid,
      offset: 0,
      limit: 25,
    })
    const group = versions.items.find((g) => g.latest.id === item.id)
    assert.equal(group.versions.length, 2)
    assert.equal(new Set(group.versions.map((d) => d.turn_run_id)).size, 2)
    if (page) {
      for (const version of group.versions) {
        await page
          .locator(`[data-deliverable-run="${version.turn_run_id}"]`)
          .waitFor({ timeout: 30000 })
      }
      report.checks.push({
        name: `${name}-inline-deliverable-associations`,
        passed: true,
        liveWithoutReload: true,
        afterRefresh: true,
        samePathMultipleRuns: true,
        externalDelivery: true,
        reliableLightweightRefresh: true,
      })
    }

    await officeDeliveryChecks({
      api,
      f,
      cid,
      folderId,
      page,
      piModel,
      submit,
      finish,
      ready,
      snapshot,
      report,
      name,
    })

    await run(
      `BUSINESS_PI_USER PI_SCENARIO_delegate_${marker}`,
      "PI_DELEGATION_COMPLETED"
    )
    assert.ok(
      piModel.evidence().some((r) => r.childContext && r.childTask),
      "A separate Pi child must receive and complete its task"
    )
    await run(
      `BUSINESS_PI_USER PI_SCENARIO_cancel_${marker}`,
      "PI_CHILD_CANCEL_CONFIRMED"
    )

    // Provider rejection is a failed task, never an accepted/successful receipt.
    piModel.reject(true)
    const failed = await submit({})
    assert.equal((await finish(failed.id)).status, "failed")
    piModel.reject(false)
    await ready()

    // Cancellation of a queued message never cancels the owning main task.
    piModel.delay(1800)
    await start("BUSINESS_PI_USER queue cancellation owner")
    await until(
      snapshot,
      (s) => s.status === "prompting",
      "Cancellation owner active"
    )
    const cancelled = await submit({})
    assert.equal(
      (
        await api("cancel_business_message", {
          conversationId: cid,
          messageId: cancelled.id,
        })
      ).status,
      "cancelled"
    )
    await ready()
    piModel.delay(50)

    // An ordinary queued message must not starve the active receipt's deadline.
    piModel.delay(6500)
    const expiring = await submit({ deadlineSeconds: 3 })
    await until(
      snapshot,
      (s) => s.status === "prompting",
      "External deadline owner active"
    )
    const afterDeadline = await submit({ deadlineSeconds: 30 })
    assert.equal((await finish(expiring.id)).status, "cancelled")
    assert.equal((await finish(afterDeadline.id)).status, "completed")
    await ready()
    piModel.delay(50)

    const original = (await api("list_all_conversations")).find(
      (c) => c.id === cid
    ).external_id
    const branchRequest = {
      requestId: randomUUID(),
      sourceConversationId: cid,
      deferIfSourceBusy: true,
    }
    await api("queue_conversation_branch_creation", { request: branchRequest })
    const branchTask = await until(
      () =>
        api("get_conversation_branch_creation_task", {
          requestId: branchRequest.requestId,
        }),
      (t) => t && !["queued", "running"].includes(t.status),
      "Pi branch creation"
    )
    assert.equal(branchTask.status, "succeeded", branchTask.error)
    const branch = branchTask.result
    assert.equal(branch.forkMode, "native")
    assert.notEqual(branch.branchSessionId, original)
    assert.equal(
      (await api("list_all_conversations")).find((c) => c.id === cid)
        .external_id,
      original
    )
    const branchMessage = await submit({
      conversationId: branch.branchConversationId,
      blocks: [{ type: "text", text: "BUSINESS_PI_USER PI_BRANCH_ONLY" }],
    })
    const branchReceipt = await until(
      () =>
        api("get_business_message", {
          conversationId: branch.branchConversationId,
          messageId: branchMessage.id,
        }),
      terminal,
      "Pi branch task"
    )
    assert.equal(branchReceipt.status, "completed")
    const mergeRequest = {
      branchConversationId: branch.branchConversationId,
      requestId: randomUUID(),
    }
    const merge = await api("merge_conversation_branch", mergeRequest)
    const again = await api("merge_conversation_branch", mergeRequest)
    assert.equal(merge.mergeId, again.mergeId)
    assert.equal(merge.targetConversationId, cid)

    report.checks.push({
      name: `${name}-pi-native-capabilities`,
      passed: true,
      realRuntime: true,
      syntheticProvider: true,
      realAccountVerified: false,
      tiers: true,
      reasoningIndependent: true,
      nativeSteer: true,
      nativeMcp: true,
      registeredDeliverableDownloaded: true,
      childExecuted: true,
      childCancelled: true,
      nativeBranchAndReturn: true,
      sourceBindingUnchanged: true,
      externalIdempotency: true,
      externalAttachment: true,
      queuedTierApplied: true,
      externalFailureAndCancellation: true,
      externalDeadline: true,
      inlineDeliverableAssociations: true,
    })
  } finally {
    piModel.delay(50)
    piModel.reject(false)
    await api("set_delegation_settings", { settings: defaults })
  }
}
