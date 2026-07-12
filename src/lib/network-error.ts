// Recognize a transport-level network/offline failure (as opposed to a genuine,
// likely-permanent backend error) so callers can re-queue a draft for
// auto-resend on reconnect instead of dropping it. Mirrors what `WebTransport`
// throws: a `TypeError` from a failed `fetch` (offline), an `AbortError` /
// "Request timed out", or a `{ code: "network_error" }` gateway body. An
// explicit "Unauthorized" is NOT network — the auth dialog handles it.
export function isNetworkOrOfflineError(e: unknown): boolean {
  if (typeof navigator !== "undefined" && navigator.onLine === false)
    return true
  if (e instanceof TypeError) return true
  if (e && typeof e === "object") {
    if ((e as { code?: unknown }).code === "network_error") return true
    if ((e as { name?: unknown }).name === "AbortError") return true
    const msg = (e as { message?: unknown }).message
    if (typeof msg === "string") {
      if (/^\s*unauthorized\s*$/i.test(msg)) return false
      if (/timed out|failed to fetch|network|load failed/i.test(msg))
        return true
    }
  }
  return false
}
