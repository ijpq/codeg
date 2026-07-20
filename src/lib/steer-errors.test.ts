import { describe, expect, it } from "vitest"

import { classifySteerFailure } from "./steer-errors"

describe("classifySteerFailure", () => {
  it("recognizes structured and string unsupported errors", () => {
    expect(classifySteerFailure({ code: "steer_unsupported" })).toBe(
      "unsupported"
    )
    expect(classifySteerFailure("JSON-RPC -32601 method not found")).toBe(
      "unsupported"
    )
  })

  it("recognizes the expected-turn race", () => {
    expect(classifySteerFailure({ code: "no_active_steer_turn" })).toBe(
      "turn_ended"
    )
    expect(classifySteerFailure("no active turn to steer")).toBe("turn_ended")
  })

  it("leaves connection failures as other", () => {
    expect(classifySteerFailure(new Error("connection closed"))).toBe("other")
  })
})
