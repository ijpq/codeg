import { afterEach, describe, expect, it, vi } from "vitest"
import { WebTransport } from "./transport/web-transport"
import type { ConversationOutputTurnRef, MessageTurn } from "./types"

const transport = new WebTransport("http://localhost")
vi.mock("@/lib/transport", () => ({
  getTransport: () => transport,
  getShellTransport: () => transport,
  isDesktop: () => false,
  isRemoteDesktopMode: () => false,
  getActiveRemoteConnectionId: () => null,
  notifyRemoteDesktopUnauthorized: vi.fn(),
}))
import { listConversationOutputWindow } from "./api"

afterEach(() => vi.unstubAllGlobals())

describe("output refresh transport", () => {
  it("refreshes all loaded pages in bounded content-free batches", async () => {
    const turns: MessageTurn[] = Array.from({ length: 405 }, (_, i) => ({
      id: `turn-${i}`,
      role: "user",
      timestamp: "2026-10-26T00:00:00Z",
      blocks: [{ type: "text", text: "must not be sent" }],
    }))
    const references: ConversationOutputTurnRef[] = turns.map((turn) => ({
      id: turn.id,
      timestamp: turn.timestamp,
      is_user: true,
      prompt_fingerprint: `fingerprint-${turn.id}`,
    }))
    const fetchMock = vi.fn((_url: string, options: RequestInit) => {
      const body = JSON.parse(options.body as string)
      expect(body.turnRefs.length).toBeLessThanOrEqual(200)
      expect(body.turnRefs[0]).toHaveProperty("is_user", true)
      expect(JSON.stringify(body)).not.toContain("must not be sent")
      return Promise.resolve(
        new Response(
          JSON.stringify({
            artifact_runs: [],
            deliverable_runs: body.turnRefs.map(
              (ref: ConversationOutputTurnRef) => ({
                turn_run_id: `run-${ref.id}`,
                user_turn_id: ref.id,
                deliverables: [],
              })
            ),
          })
        )
      )
    })
    vi.stubGlobal("fetch", fetchMock)
    const result = await listConversationOutputWindow(7, turns, references)
    expect(fetchMock).toHaveBeenCalledTimes(3)
    expect(result.deliverable_runs).toHaveLength(405)
    expect(result.deliverable_runs[0].user_turn_id).toBe("turn-0")
    expect(result.deliverable_runs[404]?.user_turn_id).toBe("turn-404")
    expect(
      JSON.parse(fetchMock.mock.calls[0][1].body as string).turnRefs[0]
        .prompt_fingerprint
    ).toBe("fingerprint-turn-0")
  })

  it("fails the whole refresh on a partial batch failure", async () => {
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValueOnce(
          new Response(
            JSON.stringify({
              artifact_runs: [],
              deliverable_runs: [],
            })
          )
        )
        .mockResolvedValueOnce(new Response("unavailable", { status: 503 }))
    )
    const turns = Array.from({ length: 201 }, (_, i) => ({
      id: String(i),
      role: "user" as const,
      timestamp: "2026-10-26T00:00:00Z",
      blocks: [],
    }))
    await expect(listConversationOutputWindow(7, turns)).rejects.toThrow()
  })
})
