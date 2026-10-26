import { mkdir } from "node:fs/promises"
import { join, resolve } from "node:path"

// Change only the spawned fixture's environment. SHGetKnownFolderPath (used by
// Tauri/dirs) expands USERPROFILE and verifies that AppData directories exist;
// an empty profile can otherwise fail before CODEG_DATA_DIR is consulted.
export async function prepareWindowsProfile(env, profile) {
  const root = resolve(profile)
  const roaming = join(root, "AppData", "Roaming")
  const local = join(root, "AppData", "Local")
  await Promise.all(
    [roaming, local].map((path) => mkdir(path, { recursive: true }))
  )
  env.USERPROFILE = root
  env.APPDATA = roaming
  env.LOCALAPPDATA = local
}
