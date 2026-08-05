/**
 * Transient pre-accept rejection used while an ACP connection exists but the
 * requested historical session has not finished restoring/attaching yet.
 *
 * This is deliberately distinct from a transport failure: callers can safely
 * retain the draft and retry it with the same client message id once the
 * connection publishes prompt readiness.
 */
export class SessionRestorePendingError extends Error {
  readonly code = "session_restore_pending"

  constructor() {
    super("ACP session restore has not completed")
    this.name = "SessionRestorePendingError"
  }
}

export function isSessionRestorePendingError(error: unknown): boolean {
  if (error instanceof SessionRestorePendingError) return true
  if (!error || typeof error !== "object") return false
  const code = (error as { code?: unknown }).code
  const message = (error as { message?: unknown }).message
  return (
    code === "session_restore_pending" ||
    (typeof message === "string" &&
      message.includes("ACP session restore has not completed"))
  )
}

/**
 * A persisted resumable conversation must not auto-connect until its DB detail
 * has resolved. `detailLoading` alone is insufficient: on a fast tab switch
 * the runtime session can already exist with `detail=null` and
 * `detailLoading=false` for one commit. Connecting in that window starts a new
 * ACP session, after which the wrong runtime session id prevents the historical
 * restore from converging without a manual detail reload.
 */
export function shouldAwaitHistoricalSessionDetail(args: {
  usesPersistedDetailIdentity: boolean
  agentType: string
  detailLoaded: boolean
  detailLoading: boolean
  detailError: string | null
}): boolean {
  if (!args.usesPersistedDetailIdentity || args.agentType === "cline") {
    return false
  }
  if (args.detailError) return false
  return args.detailLoading || !args.detailLoaded
}
