import { describe, expect, it } from "vitest"
import type { PromptDraft } from "@/lib/types"
import {
  createPromptDeliveryState,
  draftSupportsNativeSteer,
  shouldQueuePromptFailure,
  transitionPromptDelivery,
} from "./prompt-delivery-state"

const textDraft = (text = "hello"): PromptDraft => ({
  blocks: [{ type: "text", text }],
  displayText: text,
})

describe("prompt delivery state machine", () => {
  it("advances submitting → accepted → running → persisted → completed", () => {
    let state = createPromptDeliveryState("optimistic-1", 1)
    state = transitionPromptDelivery(state, "accepted", { now: 2 })
    state = transitionPromptDelivery(state, "running", { now: 3 })
    state = transitionPromptDelivery(state, "persisted", { now: 4 })
    state = transitionPromptDelivery(state, "completed", { now: 5 })
    expect(state).toMatchObject({
      phase: "completed",
      acceptedAt: 2,
      completedAt: 5,
      error: null,
    })
  })

  it("does not regress running to accepted on a late HTTP callback", () => {
    const running = transitionPromptDelivery(
      createPromptDeliveryState("optimistic-2", 1),
      "running",
      { now: 2 }
    )
    expect(transitionPromptDelivery(running, "accepted", { now: 3 })).toBe(
      running
    )
  })

  it("keeps an ambiguous transport failure explicit instead of completed", () => {
    const failed = transitionPromptDelivery(
      createPromptDeliveryState("optimistic-3", 1),
      "failed",
      { now: 2, error: "acceptance unknown" }
    )
    expect(failed).toMatchObject({
      phase: "failed",
      error: "acceptance unknown",
    })
  })
})

describe("running-turn routing", () => {
  it("allows a pure text draft to use native steer", () => {
    expect(draftSupportsNativeSteer(textDraft())).toBe(true)
  })

  it("queues image + text for the next ordinary turn", () => {
    const draft: PromptDraft = {
      blocks: [
        { type: "text", text: "inspect this" },
        {
          type: "image",
          data: "AA==",
          mime_type: "image/png",
          uri: null,
        },
      ],
      displayText: "inspect this",
    }
    expect(draftSupportsNativeSteer(draft)).toBe(false)
  })

  it("queues an embedded image resource instead of partially steering text", () => {
    const draft: PromptDraft = {
      blocks: [
        { type: "text", text: "inspect this" },
        {
          type: "resource",
          uri: "clipboard://image.png",
          mime_type: "image/png",
          text: null,
          blob: "AA==",
        },
      ],
      displayText: "inspect this",
    }
    expect(draftSupportsNativeSteer(draft)).toBe(false)
  })
})

describe("Busy queue decision", () => {
  it("queues only an explicit pre-accept Busy response", () => {
    expect(
      shouldQueuePromptFailure({
        backendBusy: true,
        accepted: false,
        activeClientMessageId: "another-message",
        clientMessageId: "optimistic-4",
      })
    ).toBe(true)
  })

  it("does not queue after the same request was accepted", () => {
    expect(
      shouldQueuePromptFailure({
        backendBusy: true,
        accepted: true,
        activeClientMessageId: "optimistic-5",
        clientMessageId: "optimistic-5",
      })
    ).toBe(false)
  })

  it("does not queue an ambiguous network failure", () => {
    expect(
      shouldQueuePromptFailure({
        backendBusy: false,
        accepted: false,
        activeClientMessageId: null,
        clientMessageId: "optimistic-6",
      })
    ).toBe(false)
  })
})
