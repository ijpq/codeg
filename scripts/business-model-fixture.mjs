// A deterministic model endpoint behind the real Claude ACP/SDK runtime.
// It verifies transport/context, not a live model's reasoning or authentication.
import { createServer } from "node:http"
export async function startBusinessModel(agent = "claude") {
  const requests = []
  const server = createServer(async (req, res) => {
    if (req.method !== "POST") {
      res.writeHead(200, { "Content-Type": "application/json" })
      res.end(
        JSON.stringify({
          data: [
            { id: "claude-sonnet-4-6", type: "model", display_name: "Fixture" },
          ],
          has_more: false,
        })
      )
      return
    }
    const chunks = []
    for await (const chunk of req) chunks.push(chunk)
    const body = JSON.parse(Buffer.concat(chunks).toString())
    if (req.url.includes("count_tokens")) {
      res
        .writeHead(200, { "Content-Type": "application/json" })
        .end('{"input_tokens":42}')
      return
    }
    const input = JSON.stringify(body.messages ?? [])
    requests.push({
      taskState: input.includes("BUSINESS_VERIFIED_STATE"),
      memory: input.includes("BUSINESS_VERIFIED_MEMORY"),
      skill: input.includes("BUSINESS_SHARED_SKILL_BODY"),
      previousAgent: input.includes("BUSINESS_CODEX_COMPLETED"),
      fromClaude: input.includes("BUSINESS_CLAUDE_COMPLETED"),
      fromPi: input.includes("BUSINESS_PI_COMPLETED"),
      user: input.includes(`BUSINESS_${agent.toUpperCase()}_USER`),
      rules:
        input.includes("BUSINESS_SAFE_BOUNDARY") &&
        input.includes("BUSINESS_SOURCE_INSTANCE_RULES"),
      ledger: input.includes("custom-ledger.md"),
    })
    const text =
      agent === "pi"
        ? "BUSINESS_PI_COMPLETED agenda-v3.md"
        : "BUSINESS_CLAUDE_COMPLETED agenda-v2.md"
    const result = {
      id: `msg_business_${requests.length}`,
      type: "message",
      role: "assistant",
      model: body.model,
      content: [{ type: "text", text }],
      stop_reason: "end_turn",
      stop_sequence: null,
      usage: { input_tokens: 42, output_tokens: 12 },
    }
    if (!body.stream) {
      res
        .writeHead(200, { "Content-Type": "application/json" })
        .end(JSON.stringify(result))
      return
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
    })
    const events = [
      [
        "message_start",
        {
          message: {
            ...result,
            content: [],
            stop_reason: null,
            usage: { input_tokens: 42, output_tokens: 0 },
          },
        },
      ],
      [
        "content_block_start",
        { index: 0, content_block: { type: "text", text: "" } },
      ],
      [
        "content_block_delta",
        { index: 0, delta: { type: "text_delta", text } },
      ],
      ["content_block_stop", { index: 0 }],
      [
        "message_delta",
        {
          delta: { stop_reason: "end_turn", stop_sequence: null },
          usage: { output_tokens: 12 },
        },
      ],
      ["message_stop", {}],
    ]
    for (const [type, payload] of events)
      res.write(
        `event: ${type}\ndata: ${JSON.stringify({ type, ...payload })}\n\n`
      )
    res.end()
  })
  await new Promise((done) => server.listen(0, "127.0.0.1", done))
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    evidence: () => [...requests],
    close: async () => {
      server.closeAllConnections()
      await new Promise((done) => server.close(done))
    },
  }
}
