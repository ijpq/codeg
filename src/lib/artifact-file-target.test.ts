import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  statWorkspaceFile: vi.fn(),
  getHomeDirectory: vi.fn(),
}))

vi.mock("@/lib/api", () => mocks)

import { resolveAvailableArtifactPath } from "./artifact-file-target"
import { resetHomeDirCacheForTests } from "./file-open-target"

describe("resolveAvailableArtifactPath", () => {
  beforeEach(() => {
    mocks.statWorkspaceFile.mockReset().mockResolvedValue({
      path: "report.docx",
      size: 42,
      mtime_ms: 1,
    })
    mocks.getHomeDirectory.mockReset().mockResolvedValue("/home/me")
    resetHomeDirCacheForTests()
  })

  it("resolves a relative artifact inside a normal or hidden session folder", async () => {
    await expect(
      resolveAvailableArtifactPath("out/report.docx", "/data/chat/session-1")
    ).resolves.toBe("/data/chat/session-1/out/report.docx")

    expect(mocks.statWorkspaceFile).toHaveBeenCalledWith(
      "/data/chat/session-1",
      "out/report.docx"
    )
  })

  it("checks an absolute artifact outside the session by dirname and basename", async () => {
    await expect(
      resolveAvailableArtifactPath("/exports/final/report.docx", "/repo")
    ).resolves.toBe("/exports/final/report.docx")

    expect(mocks.statWorkspaceFile).toHaveBeenCalledWith(
      "/exports/final",
      "report.docx"
    )
  })

  it("expands a home-relative artifact before checking it", async () => {
    await expect(
      resolveAvailableArtifactPath("~/exports/report.docx", "/repo")
    ).resolves.toBe("/home/me/exports/report.docx")

    expect(mocks.statWorkspaceFile).toHaveBeenCalledWith(
      "/home/me/exports",
      "report.docx"
    )
  })
})
