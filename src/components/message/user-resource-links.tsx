"use client"

import { FileSearch } from "lucide-react"
import { toast } from "sonner"
import { useTranslations } from "next-intl"
import type { UserResourceDisplay } from "@/lib/adapters/ai-elements-adapter"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useWorkspaceActions } from "@/contexts/workspace-context"
import { readFilePreview } from "@/lib/api"
import { toFolderRelativePath } from "@/lib/file-path-display"
import { cn } from "@/lib/utils"

interface UserResourceLinksProps {
  resources: UserResourceDisplay[]
  className?: string
}

// A chip is openable when its `uri` is a bare filesystem path (no scheme). That
// is exactly the set of lifted "blocked" @mentions — Codex marked the path
// `[blocked]` because its sandbox couldn't read it, so codeg dropped the real
// uri and stored `uri === name`. Real attachments carry a `file://` / `codeg://`
// uri and stay non-interactive as before.
function isOpenablePath(uri: string): boolean {
  return uri.length > 0 && !/^[a-z][a-z0-9+.-]*:\/\//i.test(uri)
}

const CHIP_CLASS =
  "inline-flex items-center gap-1 rounded-full border border-border/70 bg-muted/40 px-2 py-1 text-xs text-muted-foreground"

/**
 * The attachment summary row shown beneath a user message: one grey chip per
 * attached file. Real attachments render as plain, non-interactive chips
 * (complementing the inline file badges in the prose). A "blocked" @mention —
 * where Codex's sandbox couldn't read the path — renders as a CLICKABLE chip:
 * codeg's file server runs outside that sandbox, so if the file is inside the
 * workspace we can still open a preview (and toast when it's truly unreachable,
 * e.g. outside the workspace or gone).
 */
export function UserResourceLinks({
  resources,
  className,
}: UserResourceLinksProps) {
  const t = useTranslations("Folder.chat.userResources")
  const { activeFolder: folder } = useActiveFolder()
  const { openFilePreview } = useWorkspaceActions()

  if (resources.length === 0) return null

  const openResource = async (uri: string, name: string) => {
    const folderPath = folder?.path
    // Probe existence first so an unreachable path (outside the workspace jail,
    // or gone) gives a clear toast instead of a silent no-op / opaque error tab.
    if (folderPath) {
      const rel = toFolderRelativePath(uri, folderPath)
      try {
        await readFilePreview(folderPath, rel)
      } catch {
        toast.error(t("fileUnavailable", { filePath: name }))
        return
      }
    }
    void openFilePreview(uri, { folderId: folder?.id })
  }

  return (
    <div className={className}>
      <div className="flex flex-wrap gap-1.5">
        {resources.map((resource, index) => {
          const key = `${resource.uri}-${index}`
          if (!isOpenablePath(resource.uri)) {
            return (
              <div key={key} className={CHIP_CLASS}>
                <FileSearch className="h-3 w-3" />
                <span className="max-w-56 truncate">{resource.name}</span>
              </div>
            )
          }
          return (
            <button
              key={key}
              type="button"
              onClick={() => void openResource(resource.uri, resource.name)}
              aria-label={t("openFile", { filePath: resource.name })}
              title={t("openFile", { filePath: resource.name })}
              className={cn(
                CHIP_CLASS,
                "cursor-pointer transition-colors hover:border-border hover:bg-accent/50 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
              )}
            >
              <FileSearch className="h-3 w-3" />
              <span className="max-w-56 truncate">{resource.name}</span>
            </button>
          )
        })}
      </div>
    </div>
  )
}
