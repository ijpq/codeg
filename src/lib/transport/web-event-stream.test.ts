import { describe, expect, it, vi } from "vitest"

import { WebEventStream } from "./web-event-stream"

describe("WebEventStream attach readiness", () => {
  it("marks every initial attach and reconnect attach as pending until replay", () => {
    const sent: object[] = []
    let onReady: (() => void) | undefined
    const stream = new WebEventStream({
      isWsOpen: () => true,
      sendFrame: (frame) => {
        sent.push(frame)
        return true
      },
      onWsReady: (callback) => {
        onReady = callback
        return vi.fn()
      },
    })
    const lifecycle: string[] = []
    const subscription = stream.attach(
      "connection-1",
      { sinceSeq: 3 },
      {
        onAttaching: () => lifecycle.push("attaching"),
        onSnapshot: () => lifecycle.push("snapshot"),
        onReplay: () => lifecycle.push("replay"),
        onEvent: vi.fn(),
        onDetached: vi.fn(),
      }
    )

    expect(lifecycle).toEqual(["attaching"])
    expect(sent[0]).toMatchObject({
      action: "attach",
      connection_id: "connection-1",
      since_seq: 3,
    })

    stream.handleServerFrame({
      type: "replay",
      subscription_id: subscription.subscriptionId,
      connection_id: "connection-1",
      events: [],
      high_water_seq: 7,
    })
    expect(lifecycle).toEqual(["attaching", "replay"])

    onReady?.()
    expect(lifecycle).toEqual(["attaching", "replay", "attaching"])
    expect(sent[1]).toMatchObject({
      action: "attach",
      connection_id: "connection-1",
      since_seq: 7,
    })
  })
})
