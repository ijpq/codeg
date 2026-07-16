"use client"

import { useCallback } from "react"
import { PanelRight, Settings, SquareTerminal } from "lucide-react"
import { useTranslations } from "next-intl"
import { openSettingsWindow } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"
import { useTerminalContext } from "@/contexts/terminal-context"
import { useIsActiveChatMode } from "@/hooks/use-is-active-chat-mode"
import { useIsMac } from "@/hooks/use-is-mac"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { formatShortcutLabel } from "@/lib/keyboard-shortcuts"

/**
 * The window's top-RIGHT edge cluster: terminal toggle + aux-panel toggle +
 * settings. Rendered inside whichever column occupies the window's right edge —
 * the AuxPanel header when it's open (macOS), else the right-most middle column's
 * top bar. Mirrors the right cluster that lived in the old full-width title bar
 * (folder-title-bar.tsx), preserving its disabled predicates and active styling.
 */
export function RightEdgeChrome() {
  const tTitleBar = useTranslations("Folder.folderTitleBar")
  const { activeFolder } = useActiveFolder()
  const isChatMode = useIsActiveChatMode()
  const { isOpen: auxPanelOpen, toggle: toggleAuxPanel } = useAuxPanelContext()
  const { isOpen: terminalOpen, toggle: toggleTerminal } = useTerminalContext()
  const isMac = useIsMac()
  const { shortcuts } = useShortcutSettings()

  const handleOpenSettings = useCallback(() => {
    openSettingsWindow().catch((err) => {
      console.error("[RightEdgeChrome] failed to open settings:", err)
    })
  }, [])

  return (
    <div className="flex h-full shrink-0 items-center gap-2 pl-1">
      <Button
        variant="ghost"
        size="icon"
        className={`h-6 w-6 hover:text-foreground/80 ${terminalOpen ? "bg-accent" : ""}`}
        onClick={() => toggleTerminal()}
        disabled={!activeFolder}
        title={tTitleBar("withShortcut", {
          label: tTitleBar("toggleTerminal"),
          shortcut: formatShortcutLabel(shortcuts.toggle_terminal, isMac),
        })}
      >
        <SquareTerminal className="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className={`h-6 w-6 hover:text-foreground/80 ${auxPanelOpen ? "bg-accent" : ""}`}
        onClick={toggleAuxPanel}
        disabled={!activeFolder && !isChatMode}
        title={tTitleBar("withShortcut", {
          label: tTitleBar("toggleAuxPanel"),
          shortcut: formatShortcutLabel(shortcuts.toggle_aux_panel, isMac),
        })}
      >
        <PanelRight className="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="h-6 w-6 hover:text-foreground/80"
        onClick={handleOpenSettings}
        title={tTitleBar("withShortcut", {
          label: tTitleBar("openSettings"),
          shortcut: formatShortcutLabel(shortcuts.open_settings, isMac),
        })}
      >
        <Settings className="h-3.5 w-3.5" />
      </Button>
    </div>
  )
}
