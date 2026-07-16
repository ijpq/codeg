"use client"

import { useCallback, useEffect, useState, type ReactNode } from "react"
import { Folder, FolderPen, GitCommit, Info } from "lucide-react"
import { useTranslations } from "next-intl"
import {
  useAuxPanelContext,
  type AuxPanelTab,
} from "@/contexts/aux-panel-context"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useIsActiveChatMode } from "@/hooks/use-is-active-chat-mode"
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs"
import { SessionDetailsTab } from "./aux-panel-session-details-tab"
import { FileTreeTab } from "./aux-panel-file-tree-tab"
import { GitChangesTab } from "./aux-panel-git-changes-tab"
import { GitLogTab } from "./aux-panel-git-log-tab"

const LAZY_TABS: AuxPanelTab[] = ["file_tree", "changes", "git_log"]

/**
 * Decide which aux-panel tabs are available and which to actually show.
 *
 * The folder-scoped tabs (files/changes/commits) only make sense with a real
 * folder workspace open, so chat sessions and the folderless state collapse to
 * just the Session Details tab. `effectiveTab` keeps the rendered selection
 * valid even when the stored `activeTab` is a now-hidden folder tab, avoiding a
 * one-frame flash before the reconciling effect corrects the stored value.
 */
export function resolveAuxTabView(
  activeTab: AuxPanelTab,
  activeFolderId: number | null,
  isChatMode: boolean
): { showFolderTabs: boolean; effectiveTab: AuxPanelTab } {
  const showFolderTabs = activeFolderId != null && !isChatMode
  return {
    showFolderTabs,
    effectiveTab: showFolderTabs ? activeTab : "session_details",
  }
}

export function AuxPanel({
  headerChrome,
  reserveWindowControls = false,
}: {
  /** Desktop-only right-edge window chrome (terminal + aux + settings) injected
   *  at the right of the tab row when the aux panel owns the window's right edge
   *  (macOS). Absent on mobile / Windows-Linux. */
  headerChrome?: ReactNode
  /** Reserve trailing width for the Windows/Linux caption buttons that overlay
   *  the window's top-right corner above this header. */
  reserveWindowControls?: boolean
} = {}) {
  const t = useTranslations("Folder.auxPanel.tabs")
  const tDetails = useTranslations("Folder.sessionDetails")
  const { isOpen, activeTab, setActiveTab } = useAuxPanelContext()
  const { activeFolderId } = useActiveFolder()
  const isChatMode = useIsActiveChatMode()
  const [mountedTabs, setMountedTabs] = useState<Set<AuxPanelTab>>(
    () => new Set(LAZY_TABS.filter((tab) => tab === activeTab))
  )

  const { showFolderTabs, effectiveTab } = resolveAuxTabView(
    activeTab,
    activeFolderId,
    isChatMode
  )

  // Ensure the shown tab is mounted (covers both user clicks and programmatic changes)
  if (
    isOpen &&
    LAZY_TABS.includes(effectiveTab) &&
    !mountedTabs.has(effectiveTab)
  ) {
    setMountedTabs((prev) => new Set(prev).add(effectiveTab))
  }

  // Reconcile the stored selection when folder tabs disappear (e.g. entering a
  // chat session), so other consumers of `activeTab` stay in sync with what's
  // shown. Done in an effect — never a render-time setState on the provider.
  useEffect(() => {
    if (!showFolderTabs && activeTab !== "session_details") {
      setActiveTab("session_details")
    }
  }, [showFolderTabs, activeTab, setActiveTab])

  const handleTabValueChange = useCallback(
    (value: string) => {
      setActiveTab(value as AuxPanelTab)
    },
    [setActiveTab]
  )

  if (!isOpen) return null

  return (
    <aside className="group/aux-panel flex h-full min-h-0 flex-col overflow-hidden bg-sidebar text-sidebar-foreground select-none">
      <Tabs
        value={effectiveTab}
        onValueChange={handleTabValueChange}
        className="flex h-full flex-col gap-0"
      >
        <TabsList
          variant="line"
          className="h-10 w-full shrink-0 justify-start border-b border-border px-3 group-data-horizontal/tabs:h-10"
        >
          <TabsTrigger
            value="session_details"
            title={tDetails("menuLabel")}
            aria-label={tDetails("menuLabel")}
          >
            <Info className="h-3.5 w-3.5" />
          </TabsTrigger>
          {showFolderTabs && (
            <>
              <TabsTrigger
                value="file_tree"
                title={t("files")}
                aria-label={t("files")}
              >
                <Folder className="h-3.5 w-3.5" />
              </TabsTrigger>
              <TabsTrigger
                value="changes"
                title={t("changes")}
                aria-label={t("changes")}
              >
                <FolderPen className="h-3.5 w-3.5" />
              </TabsTrigger>
              <TabsTrigger
                value="git_log"
                title={t("commits")}
                aria-label={t("commits")}
              >
                <GitCommit className="h-3.5 w-3.5" />
              </TabsTrigger>
            </>
          )}
          {/* Right-edge window chrome + Win/Linux caption reservation. The
              draggable filler pushes them flush right and lets this header —
              now the window's top edge — move the window. */}
          <div data-tauri-drag-region className="h-full min-w-0 flex-1" />
          {headerChrome}
          {reserveWindowControls && (
            <div data-tauri-drag-region className="h-full w-[138px] shrink-0" />
          )}
        </TabsList>

        <TabsContent
          value="session_details"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          <SessionDetailsTab />
        </TabsContent>
        <TabsContent
          value="file_tree"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          {mountedTabs.has("file_tree") ? <FileTreeTab /> : null}
        </TabsContent>
        <TabsContent
          value="changes"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          {mountedTabs.has("changes") ? <GitChangesTab /> : null}
        </TabsContent>
        <TabsContent
          value="git_log"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          {mountedTabs.has("git_log") ? <GitLogTab /> : null}
        </TabsContent>
      </Tabs>
    </aside>
  )
}
