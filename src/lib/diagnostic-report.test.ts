import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({
  getLogSettings: vi.fn(),
  getRecentLogs: vi.fn(),
  listLogFiles: vi.fn(),
}))

vi.mock("@/lib/export-conversation", () => ({
  saveTextFile: vi.fn(),
}))

vi.mock("@/lib/platform", () => ({
  isDesktop: vi.fn(),
}))

vi.mock("@/lib/transport", () => ({
  isRemoteDesktopMode: vi.fn(),
}))

vi.mock("@/lib/updater", () => ({
  getCurrentAppVersion: vi.fn(),
}))

import {
  buildDiagnosticReport,
  exportDiagnosticReport,
  redactDiagnosticText,
  sanitizeLogRecordForExport,
} from "./diagnostic-report"
import { getLogSettings, getRecentLogs, listLogFiles } from "@/lib/api"
import { saveTextFile } from "@/lib/export-conversation"
import { isDesktop } from "@/lib/platform"
import { isRemoteDesktopMode } from "@/lib/transport"
import type { LogRecord } from "@/lib/types"
import { getCurrentAppVersion } from "@/lib/updater"

const mockGetSettings = vi.mocked(getLogSettings)
const mockGetRecent = vi.mocked(getRecentLogs)
const mockListFiles = vi.mocked(listLogFiles)
const mockSaveTextFile = vi.mocked(saveTextFile)
const mockIsDesktop = vi.mocked(isDesktop)
const mockIsRemote = vi.mocked(isRemoteDesktopMode)
const mockGetVersion = vi.mocked(getCurrentAppVersion)

function record(extra: Partial<LogRecord> = {}): LogRecord {
  return {
    seq: 1,
    timestamp_ms: 1,
    level: "INFO",
    target: "codeg_lib::acp",
    message: "connected",
    fields: {},
    spans: [],
    ...extra,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  mockGetVersion.mockResolvedValue("0.20.2")
  mockGetSettings.mockResolvedValue({
    level: "info",
    targets: [],
    env_locked: false,
  })
  mockListFiles.mockResolvedValue([
    { name: "codeg.2026-07-17.log", size_bytes: 123, modified_ms: 456 },
  ])
  mockGetRecent.mockResolvedValue([record()])
  mockIsDesktop.mockReturnValue(true)
  mockIsRemote.mockReturnValue(false)
  mockSaveTextFile.mockResolvedValue("saved")
})

describe("diagnostic redaction", () => {
  it("redacts credential shapes in free-form messages", () => {
    const value = redactDiagnosticText(
      'Authorization: Bearer abcdefghijklmnop api_key=sk-abcdefghijklmnop "password":"hunter2" https://x.test/?token=secret123'
    )

    expect(value).not.toContain("abcdefghijklmnop")
    expect(value).not.toContain("hunter2")
    expect(value).not.toContain("secret123")
    expect(value).toContain("[REDACTED]")
  })

  it("redacts sensitive structured fields but preserves token counters", () => {
    const sanitized = sanitizeLogRecordForExport(
      record({
        fields: {
          api_key: "sk-private-value",
          server_token: "server-private-value",
          total_tokens: "1234",
          input_tokens: "234",
        },
        spans: [
          {
            name: "http",
            fields: { authorization: "Bearer private-value", task_id: "t1" },
          },
        ],
      })
    )

    expect(sanitized.fields.api_key).toBe("[REDACTED]")
    expect(sanitized.fields.server_token).toBe("[REDACTED]")
    expect(sanitized.fields.total_tokens).toBe("1234")
    expect(sanitized.fields.input_tokens).toBe("234")
    expect(sanitized.spans[0].fields.authorization).toBe("[REDACTED]")
    expect(sanitized.spans[0].fields.task_id).toBe("t1")
  })
})

describe("buildDiagnosticReport", () => {
  it("collects version, runtime, settings, file metadata, and recent logs", async () => {
    mockIsRemote.mockReturnValue(true)

    const report = await buildDiagnosticReport()

    expect(report.schemaVersion).toBe(1)
    expect(report.app).toEqual({
      name: "Codeg",
      version: "0.20.2",
      runtime: "remote-desktop",
    })
    expect(report.logging.settings?.level).toBe("info")
    expect(report.logging.files[0].name).toBe("codeg.2026-07-17.log")
    expect(report.logging.recentRecords).toHaveLength(1)
    expect(report.privacy.excludes).toContain("conversation bodies")
    expect(report.collectionWarnings).toEqual([])
  })

  it("still builds a partial report when one diagnostic source fails", async () => {
    mockGetRecent.mockRejectedValue(
      new Error("request failed token=super-secret-token")
    )

    const report = await buildDiagnosticReport()

    expect(report.logging.recentRecords).toEqual([])
    expect(report.collectionWarnings).toHaveLength(1)
    expect(report.collectionWarnings[0]).not.toContain("super-secret-token")
    expect(report.collectionWarnings[0]).toContain("[REDACTED]")
  })
})

describe("exportDiagnosticReport", () => {
  it("saves a pretty-printed JSON report through the shared platform flow", async () => {
    await expect(exportDiagnosticReport()).resolves.toBe("saved")

    expect(mockSaveTextFile).toHaveBeenCalledTimes(1)
    const options = mockSaveTextFile.mock.calls[0][0]
    expect(options.suggestedName).toMatch(
      /^codeg-diagnostics-\d{8}T\d{6}Z\.json$/
    )
    expect(options).toMatchObject({
      mimeType: "application/json",
      filterName: "JSON",
      ext: "json",
    })
    expect(JSON.parse(options.content).app.version).toBe("0.20.2")
  })
})
