export type SteerFailureKind = "unsupported" | "turn_ended" | "other"

function errorParts(error: unknown): { code: string; message: string } {
  if (typeof error === "string") {
    return { code: "", message: error.toLowerCase() }
  }
  if (error && typeof error === "object") {
    const code = (error as { code?: unknown }).code
    const message = (error as { message?: unknown }).message
    return {
      code: typeof code === "string" ? code.toLowerCase() : "",
      message: typeof message === "string" ? message.toLowerCase() : "",
    }
  }
  return { code: "", message: String(error).toLowerCase() }
}

/** Normalize native-steer failures across Tauri's string errors and the web
 * transport's structured AppCommandError. */
export function classifySteerFailure(error: unknown): SteerFailureKind {
  const { code, message } = errorParts(error)
  if (
    code === "steer_unsupported" ||
    message.includes("native turn steering is not supported") ||
    message.includes("method not found") ||
    message.includes("-32601")
  ) {
    return "unsupported"
  }
  if (
    code === "no_active_steer_turn" ||
    message.includes("no active turn to steer") ||
    message.includes("codeg_steer_no_active_turn") ||
    message.includes("expectedturnid") ||
    message.includes("expected turn")
  ) {
    return "turn_ended"
  }
  return "other"
}
