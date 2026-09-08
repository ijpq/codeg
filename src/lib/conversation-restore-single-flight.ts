/**
 * Coalesce concurrent restore requests for one persisted conversation. The
 * backend remains the authority for connection reuse; this prevents separate
 * keep-alive views/context keys in one browser from issuing duplicate HTTP
 * restores during the same React effect window.
 */
export class ConversationRestoreSingleFlight<T> {
  /** Module-global so multiple provider instances in the same renderer share
   * one request. The backend remains the cross-window/process authority. */
  private static readonly sharedFlights = new Map<number, Promise<unknown>>()

  run(conversationId: number, start: () => Promise<T>): Promise<T> {
    const existing = ConversationRestoreSingleFlight.sharedFlights.get(
      conversationId
    ) as Promise<T> | undefined
    if (existing) return existing

    const flight = start().finally(() => {
      if (
        ConversationRestoreSingleFlight.sharedFlights.get(conversationId) ===
        flight
      ) {
        ConversationRestoreSingleFlight.sharedFlights.delete(conversationId)
      }
    })
    ConversationRestoreSingleFlight.sharedFlights.set(conversationId, flight)
    return flight
  }
}
