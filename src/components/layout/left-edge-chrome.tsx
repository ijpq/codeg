"use client"

import { useCallback } from "react"
import { PanelLeft, PawPrint } from "lucide-react"
import { useTranslations } from "next-intl"
import { openSettingsWindow } from "@/lib/api"
import { getPetSettings, openPetWindow } from "@/lib/pet/api"
import { isDesktop } from "@/lib/platform"
import { Button } from "@/components/ui/button"
import { useSidebarContext } from "@/contexts/sidebar-context"
import { useIsMac } from "@/hooks/use-is-mac"
import { usePlatform } from "@/hooks/use-platform"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { formatShortcutLabel } from "@/lib/keyboard-shortcuts"
import { NewFolderDropdown } from "./new-folder-dropdown"
import { RemoteWorkspaceDropdown } from "./remote-workspace-dropdown"

/**
 * The window's top-LEFT edge cluster: sidebar toggle + new-folder + remote +
 * pet. Rendered inside whichever column occupies the window's left edge — the
 * Sidebar header when the sidebar is open, else the conversation column's top
 * bar. `reserveMacInset` prepends a drag-region spacer so the native macOS
 * traffic lights (which float over the window's top-left corner) don't collide
 * with the toggle. Mirrors the left cluster that lived in the old full-width
 * title bar (folder-title-bar.tsx).
 */
export function LeftEdgeChrome({
  reserveMacInset = false,
}: {
  reserveMacInset?: boolean
}) {
  const tTitleBar = useTranslations("Folder.folderTitleBar")
  const tPet = useTranslations("Pet")
  const { isOpen, toggle } = useSidebarContext()
  const isMac = useIsMac()
  const { shortcuts } = useShortcutSettings()
  const { isMac: platformIsMac } = usePlatform()
  // The traffic lights only exist on the macOS desktop runtime (not web / not
  // Windows-Linux), so only reserve the inset there.
  const showMacInset = reserveMacInset && platformIsMac && isDesktop()

  const handleOpenPet = useCallback(async () => {
    if (!isDesktop()) return
    try {
      const settings = await getPetSettings()
      if (!settings.activePetId) {
        await openSettingsWindow("appearance")
        return
      }
      await openPetWindow()
    } catch {
      // No active pet or window error — route the user to the manager.
      try {
        await openSettingsWindow("appearance")
      } catch (err) {
        console.warn("[Pet] open settings failed:", err)
      }
    }
  }, [])

  return (
    <div className="flex h-full shrink-0 items-center">
      {showMacInset && (
        <div data-tauri-drag-region className="h-full w-[76px] shrink-0" />
      )}
      <div className="flex items-center gap-2 pr-1">
        <Button
          variant="ghost"
          size="icon"
          className="h-6 w-6 hover:text-foreground/80"
          onClick={toggle}
          title={tTitleBar("withShortcut", {
            label: tTitleBar(isOpen ? "hideSidebar" : "showSidebar"),
            shortcut: formatShortcutLabel(shortcuts.toggle_sidebar, isMac),
          })}
        >
          <PanelLeft className="h-3.5 w-3.5" />
        </Button>
        <NewFolderDropdown />
        <RemoteWorkspaceDropdown />
        <Button
          variant="ghost"
          size="icon"
          className="h-6 w-6 hover:text-foreground/80"
          onClick={handleOpenPet}
          title={tPet("manager.summon")}
        >
          <PawPrint className="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
  )
}
