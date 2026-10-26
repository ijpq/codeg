import type {
  ConversationArtifactsChanged,
  OfficeDeliveryPreview,
} from "./types"

interface Options {
  activeConversation: () => number | null
  selectionRevision: () => number
  load: (cid: number, run: string) => Promise<OfficeDeliveryPreview | null>
  open: (preview: OfficeDeliveryPreview) => void
  consumed?: Map<number, number>
  failed?: (preview: OfficeDeliveryPreview) => void
}

/** Terminal event reader, never a workspace file watcher or history replay. */
export function createOfficeDeliveryPreviewController(options: Options) {
  let disposed = false
  let epoch = 0
  const seen = options.consumed ?? new Map<number, number>()
  const newestEvent = new Map<number, string>()
  const flights = new Map<string, { again: boolean }>()
  const invalidate = () => {
    epoch += 1
  }

  async function handle(event: ConversationArtifactsChanged) {
    const cid = event.conversation_id
    if (disposed || options.activeConversation() !== cid) return
    newestEvent.set(cid, event.turn_run_id)
    const key = `${cid}:${event.turn_run_id}`
    const existing = flights.get(key)
    if (existing) {
      existing.again = true
      return
    }
    const flight = { again: false }
    flights.set(key, flight)
    const revision = options.selectionRevision()
    const atEpoch = epoch
    try {
      // One redundant settlement event may have arrived while an earlier
      // pre-settlement read was in flight. No timer or unbounded polling.
      for (let attempt = 0; attempt < 2; attempt += 1) {
        flight.again = false
        const preview = await options.load(cid, event.turn_run_id)
        if (!preview) {
          if (flight.again) continue
          return
        }
        if (
          preview.conversation_id !== cid ||
          preview.turn_run_id !== event.turn_run_id
        )
          return
        if (preview.run_sequence <= (seen.get(cid) ?? 0)) return
        seen.set(cid, preview.run_sequence)
        if (
          disposed ||
          atEpoch !== epoch ||
          options.activeConversation() !== cid ||
          options.selectionRevision() !== revision ||
          newestEvent.get(cid) !== event.turn_run_id
        )
          return
        if (
          preview.status === "ready" &&
          preview.snapshot_path &&
          preview.original_path
        ) {
          options.open(preview)
        } else if (preview.status === "failed") {
          options.failed?.(preview)
        }
        return
      }
    } catch {
      // Keep the task complete and the existing review visible. The verified
      // output card remains the manual retry/download entry point.
    } finally {
      flights.delete(key)
    }
  }

  return {
    handle,
    invalidate,
    dispose: () => {
      disposed = true
      invalidate()
    },
  }
}
