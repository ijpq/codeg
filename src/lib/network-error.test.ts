import { afterEach, describe, expect, it, vi } from "vitest"
import { isNetworkOrOfflineError } from "./network-error"

describe("isNetworkOrOfflineError", () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it("recognizes the WebTransport network/offline throw shapes", () => {
    // fetch offline throws a TypeError
    expect(isNetworkOrOfflineError(new TypeError("Failed to fetch"))).toBe(true)
    // per-call timeout throws Error("Request timed out")
    expect(isNetworkOrOfflineError(new Error("Request timed out"))).toBe(true)
    // AbortError DOMException
    const abort = new Error("aborted")
    abort.name = "AbortError"
    expect(isNetworkOrOfflineError(abort)).toBe(true)
    // gateway non-JSON body
    expect(
      isNetworkOrOfflineError({ code: "network_error", message: "HTTP 502" })
    ).toBe(true)
    // Safari phrasing
    expect(isNetworkOrOfflineError(new Error("Load failed"))).toBe(true)
  })

  it("does NOT treat auth or genuine backend errors as network", () => {
    expect(isNetworkOrOfflineError(new Error("Unauthorized"))).toBe(false)
    // a structured backend AppCommandError with a specific code
    expect(
      isNetworkOrOfflineError({ code: "turn_in_progress", message: "busy" })
    ).toBe(false)
    expect(
      isNetworkOrOfflineError({ code: "invalid_input", message: "bad" })
    ).toBe(false)
    expect(isNetworkOrOfflineError(null)).toBe(false)
    expect(isNetworkOrOfflineError("some string")).toBe(false)
  })

  it("treats anything as network when the browser reports offline", () => {
    vi.stubGlobal("navigator", { onLine: false })
    // even an otherwise-unclassifiable error counts while offline
    expect(isNetworkOrOfflineError({ code: "invalid_input" })).toBe(true)
    expect(isNetworkOrOfflineError(new Error("whatever"))).toBe(true)
  })
})
