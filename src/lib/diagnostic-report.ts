import { getLogSettings, getRecentLogs, listLogFiles } from "@/lib/api"
import { saveTextFile, type ExportResult } from "@/lib/export-conversation"
import { isDesktop } from "@/lib/platform"
import { isRemoteDesktopMode } from "@/lib/transport"
import type { LogFileInfo, LogRecord, LogSettingsView } from "@/lib/types"
import { getCurrentAppVersion } from "@/lib/updater"

const DIAGNOSTIC_SCHEMA_VERSION = 1
const DIAGNOSTIC_LOG_LIMIT = 5_000
const REDACTED = "[REDACTED]"

export type DiagnosticRuntime = "desktop" | "remote-desktop" | "web"

export interface DiagnosticReport {
  schemaVersion: number
  generatedAt: string
  app: {
    name: "Codeg"
    version: string
    runtime: DiagnosticRuntime
  }
  client: {
    userAgent: string | null
    language: string | null
    languages: string[]
    timezone: string | null
    online: boolean | null
  }
  logging: {
    settings: LogSettingsView | null
    files: LogFileInfo[]
    recentRecords: LogRecord[]
    recentRecordsLimit: number
    mayBeTruncated: boolean
  }
  privacy: {
    excludes: string[]
    warning: string
  }
  collectionWarnings: string[]
}

function isSensitiveFieldName(name: string): boolean {
  const normalized = name.toLowerCase().replace(/[-.]/g, "_")
  // Do not blanket-match "token": counters such as total_tokens are useful
  // diagnostics and are not secrets. Singular `_token` names, however, carry
  // credentials (`server_token`, `delegation_token`, etc.).
  if (normalized === "token" || /_token$/.test(normalized)) return true
  return /(?:^|_)(?:api_?key|authorization|password|passwd|passphrase|secret|cookie|credential|credentials|capability|cap)(?:$|_)/.test(
    normalized
  )
}

/** Redact common credential shapes from free-form log messages. This is a
 * defense-in-depth convenience, not a proof that arbitrary Debug/Trace text is
 * free of private data; the UI still asks users to review before sharing. */
export function redactDiagnosticText(value: string): string {
  return value
    .replace(
      /("(?:api[_-]?key|access[_-]?token|refresh[_-]?token|auth[_-]?token|password|passwd|passphrase|authorization|client[_-]?secret|cookie|credential)s?"\s*:\s*")[^"]*(")/gi,
      `$1${REDACTED}$2`
    )
    .replace(
      /(\b(?:CODEG_TOKEN|API_KEY|ACCESS_TOKEN|REFRESH_TOKEN|AUTH_TOKEN|PASSWORD|PASSPHRASE|CLIENT_SECRET)\s*=\s*)(?:"[^"]*"|'[^']*'|[^\s,;]+)/gi,
      `$1${REDACTED}`
    )
    .replace(
      /(\b(?:(?:server|delegation|github|codeg|api|access|refresh|auth)[_-]?token|token|authorization|api[_-]?key|password|passphrase|client[_-]?secret|secret|cookie|credential)\s*[:=]\s*)(?:bearer\s+)?[^\s,;]+/gi,
      `$1${REDACTED}`
    )
    .replace(/\bBearer\s+[A-Za-z0-9._~+/=-]{8,}/gi, `Bearer ${REDACTED}`)
    .replace(
      /([?&](?:api[_-]?key|access[_-]?token|refresh[_-]?token|auth[_-]?token|token|password)=)[^&#\s]+/gi,
      `$1${REDACTED}`
    )
    .replace(
      /\b(?:sk-(?:ant-)?[A-Za-z0-9_-]{12,}|ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|AIza[A-Za-z0-9_-]{20,})\b/g,
      REDACTED
    )
}

function sanitizeFields(
  fields: Record<string, string>
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(fields).map(([key, value]) => [
      key,
      isSensitiveFieldName(key) ? REDACTED : redactDiagnosticText(value),
    ])
  )
}

export function sanitizeLogRecordForExport(record: LogRecord): LogRecord {
  return {
    ...record,
    target: redactDiagnosticText(record.target),
    message: redactDiagnosticText(record.message),
    fields: sanitizeFields(record.fields ?? {}),
    spans: (record.spans ?? []).map((span) => ({
      ...span,
      name: redactDiagnosticText(span.name),
      fields: sanitizeFields(span.fields),
    })),
  }
}

function currentRuntime(): DiagnosticRuntime {
  if (isRemoteDesktopMode()) return "remote-desktop"
  return isDesktop() ? "desktop" : "web"
}

function readClientInfo(): DiagnosticReport["client"] {
  if (typeof navigator === "undefined") {
    return {
      userAgent: null,
      language: null,
      languages: [],
      timezone: null,
      online: null,
    }
  }

  let timezone: string | null = null
  try {
    timezone = Intl.DateTimeFormat().resolvedOptions().timeZone ?? null
  } catch {
    // A hardened browser may deny Intl environment details.
  }

  return {
    userAgent: navigator.userAgent || null,
    language: navigator.language || null,
    languages: Array.from(navigator.languages ?? []),
    timezone,
    online: typeof navigator.onLine === "boolean" ? navigator.onLine : null,
  }
}

function settledValue<T>(
  label: string,
  result: PromiseSettledResult<T>,
  fallback: T,
  warnings: string[]
): T {
  if (result.status === "fulfilled") return result.value
  const detail =
    result.reason instanceof Error
      ? result.reason.message
      : String(result.reason ?? "unknown error")
  warnings.push(`${label}: ${redactDiagnosticText(detail)}`)
  return fallback
}

export async function buildDiagnosticReport(): Promise<DiagnosticReport> {
  const results = await Promise.allSettled([
    getCurrentAppVersion(),
    getLogSettings(),
    listLogFiles(),
    getRecentLogs({ limit: DIAGNOSTIC_LOG_LIMIT }),
  ] as const)
  const warnings: string[] = []

  const version = settledValue("appVersion", results[0], "unknown", warnings)
  const settings = settledValue<LogSettingsView | null>(
    "logSettings",
    results[1],
    null,
    warnings
  )
  const files = settledValue<LogFileInfo[]>(
    "logFiles",
    results[2],
    [],
    warnings
  )
  const records = settledValue<LogRecord[]>(
    "recentLogs",
    results[3],
    [],
    warnings
  ).map(sanitizeLogRecordForExport)

  return {
    schemaVersion: DIAGNOSTIC_SCHEMA_VERSION,
    generatedAt: new Date().toISOString(),
    app: {
      name: "Codeg",
      version,
      runtime: currentRuntime(),
    },
    client: readClientInfo(),
    logging: {
      settings,
      files,
      recentRecords: records,
      recentRecordsLimit: DIAGNOSTIC_LOG_LIMIT,
      mayBeTruncated: records.length >= DIAGNOSTIC_LOG_LIMIT,
    },
    privacy: {
      excludes: [
        "conversation bodies",
        "agent transcripts",
        "database contents",
        "stored API keys and access tokens",
        "workspace file contents",
      ],
      warning:
        "Common credential patterns are redacted, but logs can still contain file paths or user-provided text. Review this report before sharing it.",
    },
    collectionWarnings: warnings,
  }
}

function diagnosticFilename(now: Date): string {
  const stamp = now
    .toISOString()
    .replace(/[-:]/g, "")
    .replace(/\.\d{3}Z$/, "Z")
  return `codeg-diagnostics-${stamp}.json`
}

/** Collect a fresh report and hand it to the platform-appropriate save flow. */
export async function exportDiagnosticReport(): Promise<ExportResult> {
  const report = await buildDiagnosticReport()
  return saveTextFile({
    content: `${JSON.stringify(report, null, 2)}\n`,
    suggestedName: diagnosticFilename(new Date()),
    mimeType: "application/json",
    filterName: "JSON",
    ext: "json",
  })
}
