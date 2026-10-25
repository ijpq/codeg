import { createRoot } from "react-dom/client"
import { NextIntlClientProvider } from "next-intl"
import type { FileWorkspaceTab } from "../../src/contexts/workspace-context"
import { FileWorkspaceTabBar } from "../../src/components/files/file-workspace-tab-bar"
import messages from "../../src/i18n/messages/en.json"
import "../../src/app/globals.css"

// Only host services are substituted. The strip, Motion layout, menus, CSS and
// browser hit testing are the production implementations.
let width = 84
let tabs: FileWorkspaceTab[] = []
let activeId: string | null = null
const closed: string[] = []
const root = createRoot(document.getElementById("root")!)

declare global {
  interface Window {
    fileTabsFixture: {
      configure: (width: number, count: number, active: number) => void
      closed: string[]
    }
  }
}

function render() {
  root.render(
    <NextIntlClientProvider locale="en" messages={messages}>
      <div className="flex h-10 bg-muted" style={{ width }}>
        <FileWorkspaceTabBar />
      </div>
    </NextIntlClientProvider>
  )
}

export function useWorkspaceFileTabs() {
  return {
    fileTabs: tabs,
    activeFileTabId: activeId,
    previewFileTabIds: new Set<string>(),
  }
}

export function useWorkspaceView() {
  return { mode: "fusion", filesMaximized: false }
}

export function useWorkspaceActions() {
  return {
    switchFileTab(id: string) {
      activeId = id
      render()
    },
    closeFileTab(id: string) {
      closed.push(id)
      tabs = tabs.filter((tab) => tab.id !== id)
      activeId = tabs[tabs.length - 1]?.id ?? null
      render()
    },
    closeOtherFileTabs() {},
    closeAllFileTabs() {},
    reorderFileTabs() {},
    toggleFilesMaximized() {},
    openFilePreview() {},
    openBrowserTab() {},
  }
}

export const isDesktop = () => true
export const isRemoteDesktopMode = () => false
export const openFileDialog = async () => null
export const browserListServices = async () => []
export const useBrowserCapabilities = () => ({ available: true })
export const useBrowserTabState = () => null
export const AGENT_MARK = ""

window.fileTabsFixture = {
  closed,
  configure(nextWidth, count, active) {
    width = nextWidth
    tabs = Array.from({ length: count }, (_, index) => ({
      id: `file:/fixture/office-review-${index}.docx`,
      kind: "file",
      folderId: null,
      path: `/fixture/office-review-${index}.docx`,
      title: `office-review-${index}.docx`,
      description: null,
      language: "office",
      content: "",
      loading: false,
    })) as FileWorkspaceTab[]
    activeId = tabs[active]?.id ?? null
    render()
  },
}
window.fileTabsFixture.configure(width, 2, 1)
