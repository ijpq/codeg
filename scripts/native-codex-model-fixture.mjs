// Deterministic Responses endpoint used behind the actual native Codex runtime.
import { createServer } from "node:http"
const delay = (ms) => new Promise((done) => setTimeout(done, ms))
export async function startGoalModel() {
  let requests = 0
  let finishNext = false
  let ordinaryMode = false
  let businessMode = false
  const businessRequests = []
  const requestParameters = []
  let tierBehavior
  let responseDelay = 100
  const server = createServer(async (request, response) => {
    if (request.method !== "POST") {
      response.writeHead(404).end()
      return
    }
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    const body = JSON.parse(Buffer.concat(chunks).toString())
    requestParameters.push({
      model: body.model,
      serviceTier: body.service_tier ?? null,
      effort: body.reasoning?.effort ?? null,
    })
    if (tierBehavior === "reject") {
      response.writeHead(400, { "Content-Type": "application/json" }).end(
        JSON.stringify({
          error: {
            message: "Synthetic service tier rejected",
            type: "invalid_request_error",
            code: "unsupported_service_tier",
          },
        })
      )
      return
    }
    requests++
    if (businessMode) {
      const input = JSON.stringify(body.input ?? [])
      const latestUser = JSON.stringify(
        (body.input ?? []).findLast((item) => item.role === "user") ?? {}
      )
      businessRequests.push({
        composerMarker:
          [...latestUser.matchAll(/LEGACY_COMPOSER_[a-z-]+_[a-f0-9-]{36}/g)].at(
            -1
          )?.[0] ?? null,
        taskState: input.includes("BUSINESS_VERIFIED_STATE"),
        memory: input.includes("BUSINESS_VERIFIED_MEMORY"),
        skill: input.includes("BUSINESS_SHARED_SKILL_BODY"),
        previousAgent: input.includes("BUSINESS_CLAUDE_COMPLETED"),
        fromPi: input.includes("BUSINESS_PI_COMPLETED"),
        user: input.includes("BUSINESS_CODEX_RETURN"),
        rules:
          input.includes("BUSINESS_SAFE_BOUNDARY") &&
          input.includes("BUSINESS_SOURCE_INSTANCE_RULES") &&
          input.includes("BUSINESS_RULE_TAIL_DELEGATION") &&
          input.includes("小弟必须通过真实委派工具执行") &&
          input.includes("BUSINESS_RULE_TAIL_MATERIAL") &&
          input.includes("生成材料只呈现修改结果"),
        rulesRevisionB: input.includes("BUSINESS_RULE_REVISION_B_AT_TAIL"),
        ledger: input.includes("custom-ledger.md"),
      })
    }
    const text = businessMode
      ? "BUSINESS_CODEX_COMPLETED agenda-v1.md"
      : ordinaryMode
        ? "SYNTHETIC ORDINARY RESPONSE"
        : "SYNTHETIC NATIVE GOAL RESPONSE"
    let item = {
      id: `msg_${requests}`,
      type: "message",
      role: "assistant",
      status: "completed",
      content: [{ type: "output_text", text, annotations: [] }],
    }
    const result = {
      id: `resp_${requests}`,
      object: "response",
      created_at: 1791763200,
      status: "completed",
      model: body.model,
      ...(["priority", "default"].includes(tierBehavior)
        ? { service_tier: tierBehavior }
        : {}),
      output: [item],
      usage: {
        input_tokens: 42,
        output_tokens: 3,
        total_tokens: 45,
        input_tokens_details: { cached_tokens: 0 },
        output_tokens_details: { reasoning_tokens: 0 },
      },
    }
    let events = [
      {
        type: "response.created",
        response: { ...result, status: "in_progress", output: [] },
      },
      {
        type: "response.output_item.added",
        output_index: 0,
        item: { ...item, status: "in_progress", content: [] },
      },
      {
        type: "response.content_part.added",
        item_id: item.id,
        output_index: 0,
        content_index: 0,
        part: { type: "output_text", text: "", annotations: [] },
      },
      {
        type: "response.output_text.delta",
        item_id: item.id,
        output_index: 0,
        content_index: 0,
        delta: text,
      },
      {
        type: "response.output_text.done",
        item_id: item.id,
        output_index: 0,
        content_index: 0,
        text,
      },
      { type: "response.output_item.done", output_index: 0, item },
      { type: "response.completed", response: result },
    ]
    // Session title/warm-up requests may omit tools. Complete only on the
    // actual Goal execution request that carries the native update tool.
    if (finishNext) {
      const available = [
        ...(body.tools ?? []),
        ...(body.input ?? [])
          .filter((entry) => entry.type === "additional_tools")
          .flatMap((entry) => entry.tools ?? []),
      ]
      const tools = available.flatMap((tool) =>
        tool.type === "namespace"
          ? tool.tools.map((entry) => ({ ...entry, namespace: tool.name }))
          : [tool]
      )
      const tool =
        tools.find((entry) => entry.name === "update_goal") ??
        tools.find(
          (entry) =>
            entry.name === "exec" && entry.description?.includes("update_goal")
        )
      if (tool) {
        finishNext = false
        const custom = tool.type === "custom"
        item = {
          id: `fc_${requests}`,
          call_id: `complete_goal_${requests}`,
          type: custom ? "custom_tool_call" : "function_call",
          name: tool.name,
          ...(tool.namespace ? { namespace: tool.namespace } : {}),
          ...(custom
            ? { input: 'text(await tools.update_goal({status: "complete"}));' }
            : { arguments: JSON.stringify({ status: "complete" }) }),
          status: "completed",
        }
        result.output = [item]
        events = [
          {
            type: "response.created",
            response: { ...result, status: "in_progress", output: [] },
          },
          {
            type: "response.output_item.added",
            output_index: 0,
            item: { ...item, status: "in_progress" },
          },
          { type: "response.output_item.done", output_index: 0, item },
          { type: "response.completed", response: result },
        ]
      }
    }
    await delay(
      businessMode &&
        JSON.stringify(body.input ?? []).includes("BUSINESS_SLOW_BOUNDARY")
        ? 8000
        : responseDelay
    )
    if (!response.destroyed) {
      response.writeHead(200, { "Content-Type": "text/event-stream" })
      response.end(
        events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join("")
      )
    }
  })
  await new Promise((done) => server.listen(0, "127.0.0.1", done))
  return {
    url: `http://127.0.0.1:${server.address().port}/v1`,
    count: () => requests,
    parameters: () => [...requestParameters],
    tier: (value) => {
      tierBehavior = value
    },
    delay: (value) => {
      responseDelay = value
    },
    ordinary: (value) => {
      ordinaryMode = value
    },
    business: (value) => {
      businessMode = value
    },
    businessEvidence: () => [...businessRequests],
    completeNext: () => {
      finishNext = true
    },
    close: async () => {
      server.closeAllConnections()
      await new Promise((done) => server.close(done))
    },
  }
}
