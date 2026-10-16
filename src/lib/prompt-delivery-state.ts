import type { PromptDraft, PromptInputBlock } from "@/lib/types"

/**
 * Frontend delivery lifecycle for one user message. `draft` lives in the
 * composer; every state after the user presses Send is persisted in the
 * conversation runtime under the caller-stable client message id.
 */
export type PromptDeliveryPhase =
  | "draft"
  | "submitting"
  | "accepted"
  | "running"
  | "persisted"
  | "completed"
  | "failed"
  | "queued"

export interface PromptDeliveryState {
  clientMessageId: string
  phase: PromptDeliveryPhase
  submittedAt: number
  acceptedAt: number | null
  lastEventAt: number
  completedAt: number | null
  error: string | null
}

const PHASE_RANK: Record<PromptDeliveryPhase, number> = {
  draft: 0,
  queued: 1,
  submitting: 2,
  accepted: 3,
  running: 4,
  persisted: 5,
  completed: 6,
  failed: 7,
}

export function createPromptDeliveryState(
  clientMessageId: string,
  now = Date.now()
): PromptDeliveryState {
  return {
    clientMessageId,
    phase: "submitting",
    submittedAt: now,
    acceptedAt: null,
    lastEventAt: now,
    completedAt: null,
    error: null,
  }
}

/**
 * Apply a delivery event without allowing a late callback to move a confirmed
 * message backwards (for example, `onAccepted` resolving after a stream event
 * already moved it to `running`). `failed` and `queued` are explicit recovery
 * states and may be replaced once a same-id retry is accepted.
 */
export function transitionPromptDelivery(
  current: PromptDeliveryState,
  phase: Exclude<PromptDeliveryPhase, "draft">,
  options?: { now?: number; error?: string | null }
): PromptDeliveryState {
  const now = options?.now ?? Date.now()
  const recovering = current.phase === "failed" || current.phase === "queued"
  if (
    !recovering &&
    phase !== "failed" &&
    phase !== "queued" &&
    PHASE_RANK[phase] < PHASE_RANK[current.phase]
  ) {
    return current
  }
  return {
    ...current,
    phase,
    acceptedAt:
      current.acceptedAt ??
      (phase === "accepted" ||
      phase === "running" ||
      phase === "persisted" ||
      phase === "completed"
        ? now
        : null),
    lastEventAt: now,
    completedAt: phase === "completed" ? now : current.completedAt,
    error: phase === "failed" ? (options?.error ?? null) : null,
  }
}

/** Native ACP steer is deliberately text-only in Codeg's composer routing. */
export function draftSupportsNativeSteer(draft: PromptDraft): boolean {
  return (
    draft.blocks.length > 0 &&
    draft.blocks.every(
      (block: PromptInputBlock) =>
        block.type === "text" && block.text.trim().length > 0
    )
  )
}

/**
 * A queue fallback is valid only for an explicit pre-accept Busy response.
 * Ambiguous transport failures keep the optimistic message visible so a
 * reconnect can reconcile it by client_message_id instead of duplicating it.
 */
export function shouldQueuePromptFailure(args: {
  backendBusy: boolean
  accepted: boolean
  activeClientMessageId: string | null
  clientMessageId: string
}): boolean {
  return (
    args.backendBusy &&
    !args.accepted &&
    args.activeClientMessageId !== args.clientMessageId
  )
}
