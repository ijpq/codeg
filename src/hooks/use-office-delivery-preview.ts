import { useEffect } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { getOfficeDeliveryPreview } from "@/lib/api"
import { createOfficeDeliveryPreviewController } from "@/lib/office-delivery-preview"
import { getTransport } from "@/lib/transport"
import { subscribe } from "@/lib/platform"
import {
  CONVERSATION_ARTIFACTS_CHANGED_EVENT,
  type ConversationArtifactsChanged,
  type OfficeDeliveryPreview,
} from "@/lib/types"
import { useTabStore } from "@/stores/tab-store"

const consumedByBackend = new WeakMap<object, Map<number, number>>()

function activeConversation() {
  if (typeof document !== "undefined" && document.hidden) return null
  const state = useTabStore.getState()
  return (
    state.rawTabs.find((tab) => tab.id === state.activeTabId)?.conversationId ??
    null
  )
}

export function useOfficeDeliveryPreview(
  enabled: boolean,
  open: (preview: OfficeDeliveryPreview) => void,
  selectionRevision: () => number
) {
  const t = useTranslations("Folder.fileWorkspacePanel")
  useEffect(() => {
    if (!enabled) return
    const backend = getTransport()
    let consumed = consumedByBackend.get(backend)
    if (!consumed) {
      consumed = new Map()
      consumedByBackend.set(backend, consumed)
    }
    const controller = createOfficeDeliveryPreviewController({
      activeConversation,
      selectionRevision,
      open,
      consumed,
      load: getOfficeDeliveryPreview,
      failed: (preview) =>
        toast.error(t("officeReviewFailed", { name: preview.file_name ?? "" })),
    })
    // Even switching away and back before a read finishes invalidates it.
    const offTabs = useTabStore.subscribe((state, previous) => {
      if (
        state.activeTabId !== previous.activeTabId ||
        activeConversation() !==
          previous.rawTabs.find((t) => t.id === previous.activeTabId)
            ?.conversationId
      ) {
        controller.invalidate()
      }
    })
    const onVisibility = () => controller.invalidate()
    document.addEventListener("visibilitychange", onVisibility)
    let disposed = false
    let off: (() => void) | undefined
    void subscribe<ConversationArtifactsChanged>(
      CONVERSATION_ARTIFACTS_CHANGED_EVENT,
      (event) => {
        void controller.handle(event)
      }
    )
      .then((unsubscribe) => {
        if (disposed) unsubscribe()
        else off = unsubscribe
      })
      .catch(() => {})
    return () => {
      disposed = true
      controller.dispose()
      document.removeEventListener("visibilitychange", onVisibility)
      offTabs()
      off?.()
    }
  }, [enabled, open, selectionRevision, t])
}
