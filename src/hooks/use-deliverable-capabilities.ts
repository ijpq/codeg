"use client"

import { useEffect, useState } from "react"
import {
  getDeliverableCapabilities,
  type DeliverableCapabilities,
} from "@/lib/api"
import {
  getActiveRemoteConnectionId,
  getServerBaseUrl,
  isDesktop,
} from "@/lib/transport"

const cached = new Map<string, DeliverableCapabilities>()
const pending = new Map<string, Promise<DeliverableCapabilities>>()

function targetKey(): string {
  const remoteId = getActiveRemoteConnectionId()
  if (remoteId !== null) return `remote:${remoteId}`
  return isDesktop() ? "desktop:local" : `web:${getServerBaseUrl()}`
}

function loadCapabilities(key: string): Promise<DeliverableCapabilities> {
  const value = cached.get(key)
  if (value) return Promise.resolve(value)
  const existing = pending.get(key)
  if (existing) return existing
  const request = getDeliverableCapabilities().then((capabilities) => {
    cached.set(key, capabilities)
    pending.delete(key)
    return capabilities
  })
  pending.set(key, request)
  return request
}

export function useDeliverableCapabilities(): DeliverableCapabilities | null {
  const key = targetKey()
  const [loaded, setLoaded] = useState<{
    key: string
    value: DeliverableCapabilities
  } | null>(() => {
    const value = cached.get(key)
    return value ? { key, value } : null
  })
  useEffect(() => {
    let mounted = true
    void loadCapabilities(key)
      .then((value) => {
        if (mounted) setLoaded({ key, value })
      })
      .catch(() => {
        pending.delete(key)
        // A failed capability probe keeps host-only buttons hidden; the
        // downloadable operations remain available.
      })
    return () => {
      mounted = false
    }
  }, [key])
  return cached.get(key) ?? (loaded?.key === key ? loaded.value : null)
}

export function __resetDeliverableCapabilitiesForTests(): void {
  if (process.env.NODE_ENV !== "test") return
  cached.clear()
  pending.clear()
}
