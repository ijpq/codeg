import { statWorkspaceFile } from "@/lib/api"
import {
  expandHomePath,
  isHomeRelativePath,
  isPathUnderRoot,
  normalizeAbsPath,
  splitAbsPath,
} from "@/lib/file-open-target"
import {
  isAbsoluteFilePath,
  normalizeSlashPath,
  toAbsoluteFilePath,
  toFolderRelativePath,
} from "@/lib/file-path-display"

/**
 * Resolve an artifact path against the conversation's own working directory,
 * verify it without decoding its contents, and return the canonical absolute
 * path expected by the workspace tabs.
 *
 * The folder may be a normal user folder or Codeg's hidden per-chat scratch
 * folder; treating both as the session root keeps artifact opening independent
 * of whichever tab happens to be globally active.
 */
export async function resolveAvailableArtifactPath(
  filePath: string,
  folderPath?: string
): Promise<string> {
  const expanded = await expandHomePath(filePath)
  if (isHomeRelativePath(expanded)) {
    throw new Error("Unable to resolve home-relative artifact path")
  }

  const absolute = toAbsoluteFilePath(expanded, folderPath)
  if (!absolute) throw new Error("Unable to resolve artifact path")
  const normalized = normalizeAbsPath(absolute)

  // Prefer the conversation root for files inside it. Absolute artifacts
  // outside that root still use the same dirname/basename IO contract already
  // supported by openFilePreview.
  const normalizedFolder = folderPath ? normalizeAbsPath(folderPath) : null
  const inConversationRoot =
    normalizedFolder != null && isPathUnderRoot(normalized, normalizedFolder)
  const io = inConversationRoot
    ? {
        rootPath: normalizedFolder,
        ioPath: toFolderRelativePath(normalized, normalizedFolder),
      }
    : splitAbsPath(normalized)

  if (!io || isAbsoluteFilePath(normalizeSlashPath(io.ioPath))) {
    throw new Error("Unable to build artifact IO path")
  }

  await statWorkspaceFile(io.rootPath, io.ioPath)
  return normalized
}
