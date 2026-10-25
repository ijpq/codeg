import { describe, expect, it, vi } from "vitest"
import { createOfficeDeliveryPreviewController } from "./office-delivery-preview"
import type { OfficeDeliveryPreview } from "./types"

function review(run = "run-1", sequence = 1): OfficeDeliveryPreview {
  return {
    conversation_id: 12,
    turn_run_id: run,
    run_sequence: sequence,
    deliverable_id: "file-1",
    file_name: "review.docx",
    original_path: "/work/review.docx",
    snapshot_path: `/private/${run}/review.docx`,
    sha256: "hash",
    status: "ready",
    error: null,
    completed_at: "2026-10-26T00:00:00Z",
  }
}
function setup() {
  const state = { cid: 12 as number | null, revision: 0 }
  const open = vi.fn()
  const load = vi
    .fn<(cid: number, run: string) => Promise<OfficeDeliveryPreview | null>>()
    .mockResolvedValue(review())
  const controller = createOfficeDeliveryPreviewController({
    activeConversation: () => state.cid,
    selectionRevision: () => state.revision,
    load,
    open,
  })
  return { ...controller, state, load, open }
}
const event = { conversation_id: 12, turn_run_id: "run-1" }
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((r) => {
    resolve = r
  })
  return { promise, resolve }
}

describe("settled Office delivery auto-preview", () => {
  it("opens one final review and consumes duplicate events including after closing", async () => {
    const c = setup()
    await c.handle(event)
    expect(c.open).toHaveBeenCalledOnce()
    c.state.revision += 1
    await c.handle(event)
    expect(c.open).toHaveBeenCalledOnce()
  })
  it("does not load or replay deliveries of inactive conversations", async () => {
    const c = setup()
    c.state.cid = 40
    await c.handle(event)
    c.state.cid = 12
    expect(c.load).not.toHaveBeenCalled()
    expect(c.open).not.toHaveBeenCalled()
  })
  it("a pending declaration does not open; settlement uses the final replacement", async () => {
    const c = setup()
    c.load.mockResolvedValueOnce(null)
    await c.handle(event)
    expect(c.open).not.toHaveBeenCalled()
    c.load.mockResolvedValueOnce({
      ...review(),
      file_name: "final.pptx",
      snapshot_path: "/private/final.pptx",
    })
    await c.handle(event)
    expect(c.open.mock.calls[0][0].file_name).toBe("final.pptx")
  })
  it.each(["none", "failed"] as const)(
    "does not open an empty panel for %s",
    async (status) => {
      const c = setup()
      c.load.mockResolvedValue({ ...review(), status, snapshot_path: null })
      await c.handle(event)
      expect(c.open).not.toHaveBeenCalled()
    }
  )
  it("a user close or tab switch wins over an outstanding completion read", async () => {
    const c = setup()
    const read = deferred<OfficeDeliveryPreview | null>()
    c.load.mockReturnValueOnce(read.promise)
    const flight = c.handle(event)
    c.state.revision += 1
    read.resolve(review())
    await flight
    await c.handle(event)
    expect(c.open).not.toHaveBeenCalled()
  })
  it("switching away and back invalidates an old request", async () => {
    const c = setup()
    const read = deferred<OfficeDeliveryPreview | null>()
    c.load.mockReturnValueOnce(read.promise)
    const flight = c.handle(event)
    c.state.cid = 40
    c.invalidate()
    c.state.cid = 12
    read.resolve(review())
    await flight
    expect(c.open).not.toHaveBeenCalled()
  })
  it("disposal ignores late results", async () => {
    const c = setup()
    const read = deferred<OfficeDeliveryPreview | null>()
    c.load.mockReturnValueOnce(read.promise)
    const flight = c.handle(event)
    c.dispose()
    read.resolve(review())
    await flight
    expect(c.open).not.toHaveBeenCalled()
  })
  it("a delayed older completion cannot replace a newer turn", async () => {
    const c = setup()
    const read = deferred<OfficeDeliveryPreview | null>()
    c.load
      .mockReturnValueOnce(read.promise)
      .mockResolvedValueOnce(review("run-2", 2))
    const flight = c.handle(event)
    await c.handle({ ...event, turn_run_id: "run-2" })
    read.resolve(review())
    await flight
    expect(c.open).toHaveBeenCalledOnce()
    expect(c.open.mock.calls[0][0].turn_run_id).toBe("run-2")
  })
  it("a duplicate older event cannot discard a newer review being loaded", async () => {
    const c = setup()
    await c.handle(event)
    const read = deferred<OfficeDeliveryPreview | null>()
    c.load.mockReturnValueOnce(read.promise)
    const current = c.handle({ ...event, turn_run_id: "run-2" })
    await c.handle(event)
    read.resolve(review("run-2", 2))
    await current
    expect(c.open.mock.calls.map(([preview]) => preview.turn_run_id)).toEqual([
      "run-1",
      "run-2",
    ])
  })
  it("late events from any consumed turn cannot suppress the next delivery", async () => {
    const c = setup()
    await c.handle(event)
    c.load.mockResolvedValueOnce(review("run-2", 2))
    await c.handle({ ...event, turn_run_id: "run-2" })
    const read = deferred<OfficeDeliveryPreview | null>()
    c.load.mockReturnValueOnce(read.promise)
    const current = c.handle({ ...event, turn_run_id: "run-3" })
    await c.handle(event)
    read.resolve(review("run-3", 3))
    await current
    expect(c.open.mock.calls.map(([preview]) => preview.turn_run_id)).toEqual([
      "run-1",
      "run-2",
      "run-3",
    ])
  })
  it("a pending event cannot lose the settlement event arriving during its read", async () => {
    const c = setup()
    const read = deferred<OfficeDeliveryPreview | null>()
    c.load.mockReturnValueOnce(read.promise)
    const flight = c.handle(event)
    await c.handle(event)
    read.resolve(null)
    await flight
    expect(c.load).toHaveBeenCalledTimes(2)
    expect(c.open).toHaveBeenCalledOnce()
  })
  it("does not retry indefinitely or interfere with task completion on failure", async () => {
    const c = setup()
    c.load.mockRejectedValue(new Error("unavailable"))
    await c.handle(event)
    expect(c.load).toHaveBeenCalledOnce()
    expect(c.open).not.toHaveBeenCalled()
  })
  it("rejects a response belonging to a different run or business", async () => {
    const c = setup()
    c.load
      .mockResolvedValueOnce(review("other-run"))
      .mockResolvedValueOnce({ ...review(), conversation_id: 40 })
    await c.handle(event)
    await c.handle(event)
    expect(c.open).not.toHaveBeenCalled()
  })
  it("uses retained per-backend consumption when a provider or preference remounts", async () => {
    const consumed = new Map<number, number>()
    const open = vi.fn()
    const options = {
      activeConversation: () => 12,
      selectionRevision: () => 0,
      open,
      load: async () => review(),
      consumed,
    }
    const first = createOfficeDeliveryPreviewController(options)
    await first.handle(event)
    first.dispose()
    await createOfficeDeliveryPreviewController(options).handle(event)
    expect(open).toHaveBeenCalledOnce()
  })
})
