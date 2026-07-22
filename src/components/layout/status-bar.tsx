"use client"

import { StatusBarStats } from "@/components/layout/status-bar-stats"
import { StatusBarTasks } from "@/components/layout/status-bar-tasks"
import { StatusBarTokens } from "@/components/layout/status-bar-tokens"
import { StatusBarConnection } from "@/components/layout/status-bar-connection"
import { StatusBarAlerts } from "@/components/layout/status-bar-alerts"
import { StatusBarUpdate } from "@/components/layout/status-bar-update"
import { BranchDropdown } from "@/components/layout/branch-dropdown"
import { CommandDropdown } from "@/components/layout/command-dropdown"
import { useIsMobile } from "@/hooks/use-mobile"

export function StatusBar() {
  const isMobile = useIsMobile()

  if (isMobile) {
    // Mobile mirrors the desktop bar: the branch selector on the left, the
    // command launcher + context-window circle + alerts on the right. `h-8`
    // (matching desktop) gives the h-6 branch/command controls room. Branch and
    // command self-hide in chat mode / without a repo.
    return (
      <div className="h-8 shrink-0 border-t border-border ws-chrome-border ws-surface-muted px-3 flex items-center justify-between text-xs text-muted-foreground">
        <div className="flex min-w-0 items-center gap-3">
          <BranchDropdown showFolderName={false} />
        </div>
        <div className="flex items-center gap-3">
          <CommandDropdown />
          <StatusBarTokens />
          <StatusBarAlerts />
        </div>
      </div>
    )
  }

  return (
    <div className="h-8 shrink-0 border-t border-border ws-chrome-border ws-surface-muted px-4 flex items-center justify-between text-xs text-muted-foreground">
      <div className="flex items-center gap-3">
        <StatusBarStats />
        {/* Branch selector (moved from the aux "session details" tab). Folder
            name is hidden — just the branch — since the folder chip now lives in
            the conversation header. Self-hides in chat mode / without a repo. */}
        <BranchDropdown showFolderName={false} />
      </div>
      <div className="flex items-center gap-4">
        <StatusBarUpdate />
        <StatusBarTasks />
        {/* Command launcher (moved from the aux "session details" tab), taking
            the slot the old static branch label (StatusBarSessionInfo) held. */}
        <CommandDropdown />
        <StatusBarTokens />
        <StatusBarConnection />
        <StatusBarAlerts />
      </div>
    </div>
  )
}
