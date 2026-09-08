import type { CodexQuotaSnapshot } from "@/lib/types"

const PLAN_LABELS: Record<string, string> = {
  free: "Free",
  go: "Go",
  plus: "Plus",
  pro: "Pro",
  prolite: "Pro Lite",
  team: "Team",
  business: "Business",
  enterprise: "Enterprise",
  edu: "Edu",
}

/** Prefer a relay-supplied anonymous pool/tier label. A generic `codex` limit
 * name carries no package information, so fall back to Codex's plan type. */
export function codexPlanLabel(snapshot: CodexQuotaSnapshot): string {
  const limitName = snapshot.limitName?.trim()
  if (limitName && limitName.toLowerCase() !== "codex") return limitName

  const raw = snapshot.planType.trim()
  const known = PLAN_LABELS[raw.toLowerCase()]
  if (known) return known
  if (!raw) return "Codex"
  return raw
    .replace(/[_-]+/g, " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase())
}

export function quotaRemainingPercent(usedPercent: number): number {
  return Math.max(0, Math.min(100, 100 - usedPercent))
}

export function formatQuotaPercent(value: number): string {
  const rounded = Math.round(value * 10) / 10
  return `${Number.isInteger(rounded) ? rounded.toFixed(0) : rounded.toFixed(1)}%`
}
