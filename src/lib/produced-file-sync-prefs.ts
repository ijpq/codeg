// Feature: sync agent-produced files to the local PC when using the web server
// over Tailscale / LAN. Over plain HTTP the browser can't write to an arbitrary
// folder (no File System Access), so "sync" = trigger a browser download (into
// the Downloads folder) via the existing workspace-download ticket path.
//
// Two persisted pieces:
//  - `auto-download-produced` preference (default OFF — unlike the office
//    auto-preview, auto-downloading files is opt-in so we never surprise the
//    user's Downloads folder).
//  - a bounded set of already-synced file keys, so a produced file is
//    downloaded once and NOT re-downloaded every time the reply re-renders or
//    the page reloads.

import { useEffect, useState } from "react"

const AUTO_DL_KEY = "workspace:auto-download-produced"
const AUTO_DL_EVENT = "codeg:auto-download-produced-changed"

export function loadAutoDownloadProduced(): boolean {
  if (typeof window === "undefined") return false
  try {
    // Default OFF: only an explicit "true" enables it.
    return localStorage.getItem(AUTO_DL_KEY) === "true"
  } catch {
    return false
  }
}

export function saveAutoDownloadProduced(value: boolean): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(AUTO_DL_KEY, String(value))
  } catch {
    /* ignore */
  }
  // Same-window listeners (settings + workspace may share a window); other
  // windows/tabs get the native `storage` event.
  window.dispatchEvent(new CustomEvent(AUTO_DL_EVENT, { detail: value }))
}

/**
 * Reactive read of the auto-download preference. Updates live when the toggle
 * changes in this window (custom event) or another window/tab (storage event).
 */
export function useAutoDownloadProduced(): boolean {
  const [enabled, setEnabled] = useState<boolean>(loadAutoDownloadProduced)
  useEffect(() => {
    const sync = () => setEnabled(loadAutoDownloadProduced())
    window.addEventListener(AUTO_DL_EVENT, sync)
    window.addEventListener("storage", sync)
    return () => {
      window.removeEventListener(AUTO_DL_EVENT, sync)
      window.removeEventListener("storage", sync)
    }
  }, [])
  return enabled
}

// --- Dedupe: "already auto-downloaded" keys ---------------------------------
// Persisted (bounded, FIFO) so a produced file is auto-downloaded exactly once
// across re-renders AND page reloads. Without persistence, every reload would
// re-download every produced file of every visible reply.

const SYNCED_KEY = "workspace:produced-file-synced"
const SYNCED_MAX = 2000

let syncedList: string[] | null = null
let syncedSet: Set<string> | null = null

function ensureSynced(): { list: string[]; set: Set<string> } {
  if (syncedList && syncedSet) return { list: syncedList, set: syncedSet }
  let list: string[] = []
  try {
    const raw = localStorage.getItem(SYNCED_KEY)
    if (raw) {
      const parsed: unknown = JSON.parse(raw)
      if (Array.isArray(parsed)) {
        list = parsed.filter((x): x is string => typeof x === "string")
      }
    }
  } catch {
    /* ignore */
  }
  syncedList = list
  syncedSet = new Set(list)
  return { list, set: syncedSet }
}

export function hasSyncedProducedFile(key: string): boolean {
  if (typeof window === "undefined") return false
  return ensureSynced().set.has(key)
}

export function markProducedFileSynced(key: string): void {
  if (typeof window === "undefined") return
  const { list, set } = ensureSynced()
  if (set.has(key)) return
  set.add(key)
  list.push(key)
  if (list.length > SYNCED_MAX) {
    const removed = list.splice(0, list.length - SYNCED_MAX)
    for (const r of removed) set.delete(r)
  }
  try {
    localStorage.setItem(SYNCED_KEY, JSON.stringify(list))
  } catch {
    /* ignore */
  }
}
