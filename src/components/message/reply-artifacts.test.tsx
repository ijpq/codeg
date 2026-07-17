import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { ReplyArtifacts } from "./reply-artifacts"
import type { FileChangeStat } from "@/lib/session-files"
import type { FolderDetail, MessageTurn } from "@/lib/types"

const mocks = vi.hoisted(() => ({
  stableT: (key: string) => key,
  openDiff: vi.fn(),
  openFilePreview: vi.fn(),
  reveal: vi.fn(),
  extract: vi.fn(),
  statWorkspaceFile: vi.fn(),
  downloadWorkspaceFile: vi.fn(),
  getHomeDirectory: vi.fn(),
}))

vi.mock("next-intl", () => ({
  useTranslations: () => mocks.stableT,
}))
vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({
    activeFolder: { id: 99, path: "/wrong-active-folder" },
  }),
}))
vi.mock("@/contexts/workspace-context", () => ({
  useWorkspaceActions: () => ({
    openFilePreview: mocks.openFilePreview,
    openSessionFileDiff: mocks.openDiff,
  }),
}))
vi.mock("@/lib/api", () => ({
  statWorkspaceFile: mocks.statWorkspaceFile,
  downloadWorkspaceFile: mocks.downloadWorkspaceFile,
  getHomeDirectory: mocks.getHomeDirectory,
}))
vi.mock("@/lib/produced-file-sync-prefs", () => ({
  hasSyncedProducedFile: () => false,
  markProducedFileSynced: vi.fn(),
  useAutoDownloadProduced: () => false,
}))
vi.mock("@/lib/platform", () => ({
  isLocalDesktop: () => true,
  revealItemInDir: mocks.reveal,
}))
vi.mock("@/lib/session-files", () => ({
  extractReplyFileChanges: (turns: unknown) => mocks.extract(turns),
}))

const MODIFIED_DIFF =
  "diff --git a/src/a.ts b/src/a.ts\n@@ -1,2 +1,2 @@\n-old\n+new"
const DELETION_DIFF = "*** Delete File: src/gone.ts\n-a\n-b"
const sourceTurns = [{ id: "reply-turn-1" }] as unknown as MessageTurn[]

const chatFolder: FolderDetail = {
  id: 7,
  name: "Chat",
  path: "/app-data/chat-sessions/2026-07-17/session-1",
  git_branch: null,
  default_agent_type: null,
  last_opened_at: "2026-07-17T00:00:00Z",
  sort_order: 0,
  color: "inherit",
  parent_id: null,
  kind: "chat",
}

function renderCard(
  files: FileChangeStat[],
  folder?: FolderDetail | null
) {
  mocks.extract.mockReturnValue(files)
  return render(
    <ReplyArtifacts
      sourceTurns={sourceTurns}
      isResponseComplete
      folder={folder}
    />
  )
}

function expandChanged() {
  fireEvent.click(screen.getByText("title"))
}

describe("ReplyArtifacts — view diff action", () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it("opens the file's diff in the editor, keyed by the reply turn", () => {
    renderCard([
      {
        id: "f1",
        path: "src/a.ts",
        additions: 1,
        deletions: 1,
        diff: MODIFIED_DIFF,
      },
    ])
    expandChanged()

    fireEvent.click(screen.getByRole("button", { name: "viewDiff" }))

    expect(mocks.openDiff).toHaveBeenCalledWith(
      "src/a.ts",
      MODIFIED_DIFF,
      "reply-turn-1"
    )
  })

  it("falls back to the placeholder when the file has no diff data", () => {
    renderCard([
      { id: "f2", path: "src/b.ts", additions: 0, deletions: 0, diff: null },
    ])
    expandChanged()

    fireEvent.click(screen.getByRole("button", { name: "viewDiff" }))

    expect(mocks.openDiff).toHaveBeenCalledWith(
      "src/b.ts",
      "noDiffDataAvailable",
      "reply-turn-1"
    )
  })

  it("places View Diff to the left of Show-in-file-manager", () => {
    renderCard([
      {
        id: "f1",
        path: "src/a.ts",
        additions: 1,
        deletions: 1,
        diff: MODIFIED_DIFF,
      },
    ])
    expandChanged()

    const viewDiffBtn = screen.getByRole("button", { name: "viewDiff" })
    const revealBtn = screen.getByRole("button", { name: "revealInFolder" })
    expect(
      viewDiffBtn.compareDocumentPosition(revealBtn) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
  })

  it("does not offer View Diff for a removed file", () => {
    renderCard([
      {
        id: "f3",
        path: "src/gone.ts",
        additions: 0,
        deletions: 2,
        diff: DELETION_DIFF,
      },
    ])
    expandChanged()

    expect(
      screen.queryByRole("button", { name: "viewDiff" })
    ).not.toBeInTheDocument()
    expect(screen.getByText("remove")).toBeInTheDocument()
  })
})

describe("ReplyArtifacts binary artifacts", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.statWorkspaceFile.mockResolvedValue({
      path: "out/report.docx",
      size: 1_024,
      mtime_ms: 1,
    })
    mocks.getHomeDirectory.mockResolvedValue("/home/me")
  })

  it("shows a shell-created Word file and opens it from the chat folder", async () => {
    renderCard(
      [
        {
          id: "docx-1",
          path: "out/report.docx",
          additions: 0,
          deletions: 0,
          diff: null,
          created: true,
        },
      ],
      chatFolder
    )

    expect(screen.getByText("report.docx")).toBeInTheDocument()
    expect(screen.getByText("newFilesTitle")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "openFile" }))

    await waitFor(() => {
      expect(mocks.statWorkspaceFile).toHaveBeenCalledWith(
        chatFolder.path,
        "out/report.docx"
      )
      expect(mocks.openFilePreview).toHaveBeenCalledWith(
        `${chatFolder.path}/out/report.docx`,
        { folderId: chatFolder.id }
      )
    })
  })
})
