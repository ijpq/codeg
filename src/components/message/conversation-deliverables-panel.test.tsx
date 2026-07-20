import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  autoDownload: false,
  desktop: true,
  downloadWorkspaceDir: vi.fn(),
  downloadWorkspaceFile: vi.fn(),
  getHomeDirectory: vi.fn(),
  markProducedFileSynced: vi.fn(),
  openFilePreview: vi.fn(),
  statWorkspaceFile: vi.fn(),
}))

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))
vi.mock("@/contexts/workspace-context", () => ({
  useWorkspaceActions: () => ({ openFilePreview: mocks.openFilePreview }),
}))
vi.mock("@/lib/api", () => ({
  downloadWorkspaceDir: mocks.downloadWorkspaceDir,
  downloadWorkspaceFile: mocks.downloadWorkspaceFile,
  getHomeDirectory: mocks.getHomeDirectory,
  statWorkspaceFile: mocks.statWorkspaceFile,
}))
vi.mock("@/lib/produced-file-sync-prefs", () => ({
  hasSyncedProducedFile: () => false,
  markProducedFileSynced: mocks.markProducedFileSynced,
  useAutoDownloadProduced: () => mocks.autoDownload,
}))
vi.mock("@/lib/platform", () => ({
  isLocalDesktop: () => mocks.desktop,
  revealItemInDir: vi.fn(),
}))

import { ConversationDeliverablesPanel } from "./conversation-deliverables-panel"
import type { ConversationDeliverable, FolderDetail } from "@/lib/types"

const folder: FolderDetail = {
  id: 7,
  name: "Project",
  path: "/repo",
  git_branch: null,
  default_agent_type: null,
  last_opened_at: "2026-07-18T00:00:00Z",
  sort_order: 0,
  color: "inherit",
  parent_id: null,
  kind: "regular",
  alias: null,
}

const deliverables: ConversationDeliverable[] = [
  {
    id: "supporting",
    conversation_id: 1,
    root_path: "/repo",
    path: "out/assets",
    kind: "directory",
    title: "Assets",
    role: "supporting",
    position: 0,
    source: "agent_declared",
    verified_at: "2026-07-18T00:00:00Z",
    created_at: "2026-07-18T00:00:00Z",
    updated_at: "2026-07-18T00:00:00Z",
  },
  {
    id: "primary",
    conversation_id: 1,
    root_path: "/repo",
    path: "out/report.pdf",
    kind: "file",
    title: "Final report",
    description: "Ready to share",
    role: "primary",
    position: 1,
    source: "agent_declared",
    verified_at: "2026-07-18T00:00:00Z",
    created_at: "2026-07-18T00:00:00Z",
    updated_at: "2026-07-18T00:00:00Z",
  },
]

describe("ConversationDeliverablesPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.autoDownload = false
    mocks.desktop = true
    mocks.downloadWorkspaceDir.mockResolvedValue(undefined)
    mocks.downloadWorkspaceFile.mockResolvedValue(undefined)
    mocks.statWorkspaceFile.mockResolvedValue({
      path: "out/report.pdf",
      size: 1_024,
      mtime_ms: 1,
    })
  })

  it("renders only the declared set, with primary outputs first", () => {
    render(
      <ConversationDeliverablesPanel
        expanded
        onToggle={vi.fn()}
        deliverables={deliverables}
        folder={folder}
      />
    )

    const primary = screen.getByText("Final report")
    const supporting = screen.getByText("Assets")
    expect(
      primary.compareDocumentPosition(supporting) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
    expect(screen.getByText("Ready to share")).toBeInTheDocument()
    expect(screen.queryByText("package.json")).not.toBeInTheDocument()
  })

  it("opens a verified file against its persisted root", async () => {
    render(
      <ConversationDeliverablesPanel
        expanded
        onToggle={vi.fn()}
        deliverables={[deliverables[1]]}
        folder={folder}
      />
    )

    fireEvent.click(screen.getByRole("button", { name: "openFile" }))

    await waitFor(() => {
      expect(mocks.statWorkspaceFile).toHaveBeenCalledWith(
        "/repo",
        "out/report.pdf"
      )
      expect(mocks.openFilePreview).toHaveBeenCalledWith(
        "/repo/out/report.pdf",
        { folderId: folder.id }
      )
    })
  })

  it("auto-downloads verified files and directories in web mode", async () => {
    mocks.autoDownload = true
    mocks.desktop = false

    render(
      <ConversationDeliverablesPanel
        expanded
        onToggle={vi.fn()}
        deliverables={deliverables}
        folder={folder}
      />
    )

    await waitFor(() => {
      expect(mocks.downloadWorkspaceFile).toHaveBeenCalledTimes(1)
      expect(mocks.downloadWorkspaceFile).toHaveBeenCalledWith(
        "/repo",
        "out/report.pdf",
        "report.pdf"
      )
      expect(mocks.downloadWorkspaceDir).toHaveBeenCalledWith(
        "/repo",
        "out/assets",
        "assets"
      )
    })
  })
})
