import { beforeEach, describe, expect, it, vi } from "vitest"

const call = vi.fn()

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call }),
  isDesktop: () => false,
}))

import {
  getRunningServerVersion,
  readServerVersionStrict,
  waitForServerHealthy,
} from "@/lib/updater"

describe("readServerVersionStrict", () => {
  beforeEach(() => {
    call.mockReset()
  })

  it("returns the version when /health reports one", async () => {
    call.mockResolvedValueOnce({ version: "0.14.12" })
    await expect(readServerVersionStrict()).resolves.toBe("0.14.12")
  })

  it("resolves null when /health responds without a version (older server)", async () => {
    call.mockResolvedValueOnce({})
    await expect(readServerVersionStrict()).resolves.toBeNull()
  })

  it("rejects (does not swallow) when the server is unreachable", async () => {
    call.mockRejectedValueOnce(new Error("down"))
    await expect(readServerVersionStrict()).rejects.toThrow("down")
  })
})

describe("getRunningServerVersion", () => {
  beforeEach(() => {
    call.mockReset()
  })

  it("returns the version on success", async () => {
    call.mockResolvedValueOnce({ version: "1.2.3" })
    await expect(getRunningServerVersion()).resolves.toBe("1.2.3")
  })

  it("swallows a transport failure as null", async () => {
    call.mockRejectedValueOnce(new Error("down"))
    await expect(getRunningServerVersion()).resolves.toBeNull()
  })
})

describe("waitForServerHealthy", () => {
  beforeEach(() => {
    call.mockReset()
  })

  it("resolves true as soon as /health answers", async () => {
    // First poll fails (server still restarting), second succeeds.
    call.mockRejectedValueOnce(new Error("down")).mockResolvedValueOnce({})

    const healthy = await waitForServerHealthy({
      timeoutMs: 5_000,
      intervalMs: 5,
    })

    expect(healthy).toBe(true)
    expect(call).toHaveBeenCalledWith("health", {}, { timeoutMs: 4000 })
    expect(call).toHaveBeenCalledTimes(2)
  })

  it("resolves false when the server never comes back before the deadline", async () => {
    call.mockRejectedValue(new Error("down"))

    const healthy = await waitForServerHealthy({
      timeoutMs: 30,
      intervalMs: 5,
    })

    expect(healthy).toBe(false)
    expect(call.mock.calls.length).toBeGreaterThan(0)
  })
})
