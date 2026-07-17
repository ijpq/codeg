import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  openFilePreview: vi.fn(),
  statWorkspaceFile: vi.fn(),
  downloadWorkspaceFile: vi.fn(),
  getHomeDirectory: vi.fn(),
}))

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))
vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({
    activeFolder: { id: 99, path: "/wrong-active-folder" },
  }),
}))
vi.mock("@/contexts/workspace-context", () => ({
  useWorkspaceActions: () => ({ openFilePreview: mocks.openFilePreview }),
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
  revealItemInDir: vi.fn(),
}))

import { ReplyArtifacts } from "./reply-artifacts"
import type { FolderDetail, MessageTurn } from "@/lib/types"

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

const wordReply: MessageTurn[] = [
  {
    id: "assistant-1",
    role: "assistant",
    timestamp: "2026-07-17T00:00:01Z",
    blocks: [
      {
        type: "tool_use",
        tool_use_id: "shell-1",
        tool_name: "exec_command",
        input_preview: `python -c "doc.save('out/report.docx')"`,
      },
      {
        type: "tool_result",
        tool_use_id: "shell-1",
        output_preview: "",
        is_error: false,
      },
    ],
  },
]

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
    render(
      <ReplyArtifacts
        sourceTurns={wordReply}
        isResponseComplete
        folder={chatFolder}
      />
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
