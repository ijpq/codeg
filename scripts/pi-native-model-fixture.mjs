// Deterministic provider used with the actual Pi, native tools and CodeG MCP.
import { createServer } from "node:http"
const pause = (ms) => new Promise((done) => setTimeout(done, ms))
export async function startPiModel() {
  const requests = [],
    scenarios = new Map()
  let responseDelay = 50,
    rejectTier = false,
    officeHeld = false
  const officeWaiters = new Set()
  const server = createServer(async (req, res) => {
    if (req.method !== "POST") {
      res.writeHead(404).end()
      return
    }
    const chunks = []
    for await (const c of req) chunks.push(c)
    const body = JSON.parse(Buffer.concat(chunks).toString())
    const input = JSON.stringify(body.input ?? [])
    const latestUser = JSON.stringify(
      (body.input ?? []).findLast((i) => i.role === "user") ?? {}
    )
    const isChild = latestUser.includes("PI_CHILD_TASK")
    const tools = body.tools ?? []
    requests.push({
      model: body.model,
      tier: body.service_tier ?? null,
      effort: body.reasoning?.effort ?? null,
      taskState: input.includes("BUSINESS_VERIFIED_STATE"),
      memory: input.includes("BUSINESS_VERIFIED_MEMORY"),
      skill: input.includes("BUSINESS_SHARED_SKILL_BODY"),
      previousAgent: input.includes("BUSINESS_CODEX_COMPLETED"),
      fromClaude: input.includes("BUSINESS_CLAUDE_COMPLETED"),
      fromPi: input.includes("BUSINESS_PI_COMPLETED"),
      user: input.includes("BUSINESS_PI_USER"),
      rules:
        input.includes("BUSINESS_SAFE_BOUNDARY") &&
        input.includes("BUSINESS_SOURCE_INSTANCE_RULES") &&
        input.includes("BUSINESS_RULE_TAIL_DELEGATION") &&
        input.includes("小弟必须通过真实委派工具执行") &&
        input.includes("BUSINESS_RULE_TAIL_MATERIAL") &&
        input.includes("生成材料只呈现修改结果"),
      rulesRevisionB: input.includes("BUSINESS_RULE_REVISION_B_AT_TAIL"),
      ledger: input.includes("custom-ledger.md"),
      image: input.includes('"input_image"'),
      steer: input.includes("PI_NATIVE_GUIDANCE"),
      childContext: input.includes("PI_CHILD_CONTEXT"),
      childTask: isChild,
      tools: tools
        .map((t) => t.name)
        .filter((n) => /publish_deliverables|delegat/.test(n ?? "")),
    })
    if (rejectTier) {
      res.writeHead(400, { "Content-Type": "application/json" }).end(
        JSON.stringify({
          error: {
            message: "Synthetic tier rejected",
            code: "unsupported_service_tier",
          },
        })
      )
      return
    }
    const marker = [
      ...latestUser.matchAll(
        /PI_SCENARIO_(publish|delegate|cancel|office|officeclear|officeobserve)_([a-z0-9]+)/g
      ),
    ].at(-1)
    let text = isChild
      ? "PI_CHILD_COMPLETED PI_CHILD_CONTEXT"
      : "BUSINESS_PI_COMPLETED agenda-v3.md"
    let call
    if (marker && !isChild) {
      const key = marker[0],
        kind = marker[1]
      const state = scenarios.get(key) ?? { stage: 0 }
      scenarios.set(key, state)
      const choose = (name) =>
        tools.find((t) => t.name === name || t.name?.endsWith("__" + name))
          ?.name
      const tool = (name, args) => {
        const selected = choose(name)
        if (!selected) {
          text = "PI_TEST_TOOL_UNAVAILABLE " + name
          state.done = true
          return
        }
        call = { name: selected, args }
        state.stage++
      }
      const outputs = (body.input ?? [])
        .filter((i) => i.type === "function_call_output")
        .map((i) => String(i.output ?? ""))
      if (kind === "office" || kind === "officeclear") {
        const path =
          latestUser.match(/OFFICE_PATH=([a-z0-9.-]+)/)?.[1] ??
          "office-review.docx"
        if (state.stage === 0)
          tool("publish_deliverables", {
            mode: "replace",
            deliverables: [
              {
                path: "office-process.docx",
                title: "Office process draft",
                role: "primary",
              },
            ],
          })
        else if (state.stage === 1)
          tool("publish_deliverables", {
            mode: kind === "officeclear" ? "clear" : "replace",
            deliverables:
              kind === "officeclear"
                ? []
                : [{ path, title: "Office delivered review", role: "primary" }],
          })
        else {
          text = "PI_OFFICE_COMPLETED"
          state.done = true
        }
      } else if (kind === "officeobserve") {
        text = "PI_OFFICE_OBSERVED"
        state.done = true
      } else if (kind === "publish") {
        if (state.stage === 0)
          tool("write", {
            path: "pi-verified-report.md",
            content: "# Pi registered deliverable\nPI_VERIFIED_CONTENT\n",
          })
        else if (state.stage === 1)
          tool("publish_deliverables", {
            deliverables: [
              {
                path: "pi-verified-report.md",
                title: "Pi verified report",
                role: "primary",
                category: "standalone_output",
              },
            ],
          })
        else {
          text = "PI_PUBLISH_COMPLETED"
          state.done = true
        }
      } else {
        if (state.stage === 0)
          tool("delegate_to_agent", {
            agent_type: "pi",
            task:
              kind === "cancel"
                ? "PI_CHILD_TASK PI_CHILD_CONTEXT PI_CHILD_SLOW"
                : "PI_CHILD_TASK PI_CHILD_CONTEXT",
          })
        else if (!state.done) {
          const matches = [
            ...outputs
              .join(" ")
              .matchAll(/(?:task_id|taskId)[\\"\s:=]+([a-f0-9-]{20,})/g),
          ]
          const id = matches.at(-1)?.[1]
          if (!id) {
            text = "PI_TEST_MISSING_DELEGATION_ACK"
            state.done = true
          } else if (kind === "cancel" && state.stage === 1)
            tool("cancel_delegation", { task_id: id })
          else if (
            (kind === "delegate" && state.stage === 1) ||
            (kind === "cancel" && state.stage === 2)
          )
            tool("get_delegation_status", { task_ids: [id], wait_ms: 5000 })
          else if (
            outputs
              .at(-1)
              ?.includes(kind === "cancel" ? "canceled" : "PI_CHILD_COMPLETED")
          ) {
            text =
              kind === "cancel"
                ? "PI_CHILD_CANCEL_CONFIRMED"
                : "PI_DELEGATION_COMPLETED"
            state.done = true
          } else if (state.stage < 10)
            tool("get_delegation_status", { task_ids: [id], wait_ms: 5000 })
          else {
            text = "PI_TEST_CHILD_OUTCOME_NOT_CONFIRMED"
            state.done = true
          }
        }
      }
    }
    if (marker && /office/.test(marker[1]) && !call && officeHeld) {
      await new Promise((done) => officeWaiters.add(done))
    }
    const seq = requests.length
    const item = call
      ? {
          id: `fc_${seq}`,
          call_id: `call_${seq}`,
          type: "function_call",
          name: call.name,
          arguments: JSON.stringify(call.args),
          status: "completed",
        }
      : {
          id: `msg_${seq}`,
          type: "message",
          role: "assistant",
          status: "completed",
          content: [{ type: "output_text", text, annotations: [] }],
        }
    const result = {
      id: `resp_${seq}`,
      object: "response",
      created_at: 1791763200,
      status: "completed",
      model: body.model,
      service_tier: body.service_tier ?? "default",
      output: [item],
      usage: {
        input_tokens: 42,
        output_tokens: 12,
        total_tokens: 54,
        input_tokens_details: { cached_tokens: 0 },
        output_tokens_details: { reasoning_tokens: 0 },
      },
    }
    const events = [
      {
        type: "response.created",
        response: { ...result, status: "in_progress", output: [] },
      },
      {
        type: "response.output_item.added",
        output_index: 0,
        item: {
          ...item,
          status: "in_progress",
          ...(call ? {} : { content: [] }),
        },
      },
      ...(!call
        ? [
            {
              type: "response.content_part.added",
              output_index: 0,
              content_index: 0,
              item_id: item.id,
              part: { type: "output_text", text: "", annotations: [] },
            },
            {
              type: "response.output_text.delta",
              output_index: 0,
              content_index: 0,
              item_id: item.id,
              delta: text,
            },
          ]
        : []),
      { type: "response.output_item.done", output_index: 0, item },
      { type: "response.completed", response: result },
    ]
    await pause(
      isChild && latestUser.includes("PI_CHILD_SLOW") ? 15000 : responseDelay
    )
    if (!res.destroyed)
      res
        .writeHead(200, { "Content-Type": "text/event-stream" })
        .end(events.map((e) => `data: ${JSON.stringify(e)}\n\n`).join(""))
  })
  await new Promise((done) => server.listen(0, "127.0.0.1", done))
  return {
    url: `http://127.0.0.1:${server.address().port}/v1`,
    evidence: () => [...requests],
    delay: (value) => {
      responseDelay = value
    },
    reject: (value) => {
      rejectTier = value
    },
    holdOffice: (value) => {
      officeHeld = value
      if (!value) {
        for (const done of officeWaiters) done()
        officeWaiters.clear()
      }
    },
    close: async () => {
      officeHeld = false
      for (const done of officeWaiters) done()
      officeWaiters.clear()
      server.closeAllConnections()
      await new Promise((done) => server.close(done))
    },
  }
}
