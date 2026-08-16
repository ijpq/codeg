/**
 * Coalesce concurrent restore requests for one persisted conversation. The
 * backend remains the authority for connection reuse; this prevents separate
 * keep-alive views/context keys in one browser from issuing duplicate HTTP
 * restores during the same React effect window.
 */
export class ConversationRestoreSingleFlight<T> {
  private readonly flights = new Map<number, Promise<T>>()

  run(conversationId: number, start: () => Promise<T>): Promise<T> {
    const existing = this.flights.get(conversationId)
    if (existing) return existing

    const flight = start().finally(() => {
      if (this.flights.get(conversationId) === flight) {
        this.flights.delete(conversationId)
      }
    })
    this.flights.set(conversationId, flight)
    return flight
  }
}
