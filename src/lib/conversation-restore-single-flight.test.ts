import { describe, expect, it, vi } from "vitest"
import { ConversationRestoreSingleFlight } from "./conversation-restore-single-flight"

describe("ConversationRestoreSingleFlight", () => {
  it("shares one restore for concurrent callers and permits a later retry", async () => {
    let resolve!: (value: string) => void
    const start = vi.fn(
      () =>
        new Promise<string>((done) => {
          resolve = done
        })
    )
    const flights = new ConversationRestoreSingleFlight<string>()

    const first = flights.run(16, start)
    const duplicate = flights.run(16, start)
    expect(duplicate).toBe(first)
    expect(start).toHaveBeenCalledTimes(1)

    resolve("connection-1")
    await expect(first).resolves.toBe("connection-1")
    await flights.run(16, async () => "connection-2")
    expect(start).toHaveBeenCalledTimes(1)
  })

  it("does not coalesce different conversations", async () => {
    const flights = new ConversationRestoreSingleFlight<number>()
    await Promise.all([
      flights.run(1, async () => 1),
      flights.run(2, async () => 2),
    ])
  })
})
