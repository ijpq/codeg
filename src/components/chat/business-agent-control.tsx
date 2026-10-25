"use client"

import { useState } from "react"
import { useTranslations } from "next-intl"
import { getAgentLabel } from "@/lib/custom-agents"
import { toErrorMessage } from "@/lib/app-error"
import type { AgentType } from "@/lib/types"
import type { useBusinessConversation } from "@/hooks/use-business-conversation"
import {
  getBusinessMemory,
  saveBusinessMemory,
  updateBusinessTaskState,
  type BusinessTaskState,
  type SharedBusinessMemory,
} from "@/lib/business-conversation"

export function BusinessAgentControl({
  conversationId,
  folderId,
  agent,
  business,
}: {
  conversationId: number
  folderId: number
  agent: AgentType
  business: ReturnType<typeof useBusinessConversation>
}) {
  const t = useTranslations("Folder.chat.businessAgent")
  const [open, setOpen] = useState(false)
  const [task, setTask] = useState<BusinessTaskState | null>(null)
  const [revision, setRevision] = useState(0)
  const [memory, setMemory] = useState<SharedBusinessMemory[]>([])
  const [memoryScope, setMemoryScope] =
    useState<SharedBusinessMemory["scope"]>("task")
  const [memoryText, setMemoryText] = useState("")
  const [editorError, setEditorError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const state = business.state
  const toggle = async () => {
    if (open) {
      setOpen(false)
      return
    }
    if (!state) return
    setTask(structuredClone(state.taskState))
    setRevision(state.revision)
    setEditorError(null)
    try {
      const items = await getBusinessMemory(conversationId)
      setMemory(items)
      setMemoryText(
        items.find((m) => m.scope === memoryScope && m.name === "preferences")
          ?.content ?? ""
      )
      setOpen(true)
    } catch (e) {
      setEditorError(toErrorMessage(e))
    }
  }
  const save = async () => {
    if (!task) return
    setSaving(true)
    setEditorError(null)
    try {
      await updateBusinessTaskState(conversationId, revision, task)
      setRevision(revision + 1)
      const existing = memory.find(
        (m) => m.scope === memoryScope && m.name === "preferences"
      )
      if (memoryText.trim() || existing) {
        await saveBusinessMemory(conversationId, {
          scope: memoryScope,
          scopeId:
            memoryScope === "global"
              ? 0
              : memoryScope === "project"
                ? folderId
                : conversationId,
          name: "preferences",
          content: memoryText,
          source: "user",
          revision: existing?.revision ?? 0,
          updatedAt: "",
        })
      }
      await business.refresh()
      setOpen(false)
    } catch (e) {
      setEditorError(toErrorMessage(e))
      await business.refresh().catch(() => {})
    } finally {
      setSaving(false)
    }
  }
  return (
    <div className="space-y-1 text-xs" data-business-agent-control>
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-2">
          <span>{t("agent")}</span>
          <select
            aria-label={t("agent")}
            value={agent}
            disabled={business.pending}
            className="rounded border bg-background px-2 py-1"
            onChange={(e) => {
              void business.switchAgent(e.target.value as AgentType)
            }}
          >
            <option value="codex">Codex</option>
            <option value="claude_code">Claude Code</option>
          </select>
        </label>
        {state && (
          <button
            type="button"
            className="text-muted-foreground underline"
            onClick={() => {
              void toggle()
            }}
          >
            {t("sharedState")}
          </button>
        )}
        {business.pending && (
          <span role="status" className="text-muted-foreground">
            {state?.switchStatus === "queued" ? t("queued") : t("preparing")}
          </span>
        )}
        {state?.switchStatus === "failed" && (
          <button
            type="button"
            className="text-destructive underline"
            onClick={() => {
              if (state.pendingAgent)
                void business.switchAgent(state.pendingAgent)
            }}
          >
            {t("retry", { agent: getAgentLabel(state.pendingAgent ?? agent) })}
          </button>
        )}
      </div>
      {(business.error || state?.switchError || editorError) && (
        <p role="alert" className="break-words text-destructive">
          {business.error || state?.switchError || editorError}
        </p>
      )}
      {open && task && (
        <div className="grid gap-2 rounded border p-2">
          <label>
            {t("objective")}
            <input
              className="mt-1 w-full rounded border bg-background p-1"
              value={task.objective}
              onChange={(e) => setTask({ ...task, objective: e.target.value })}
            />
          </label>
          {(
            [
              "requirements",
              "completed",
              "pending",
              "latestFiles",
              "decisions",
            ] as const
          ).map((key) => (
            <label key={key}>
              {t(key)}
              <textarea
                rows={2}
                className="mt-1 w-full rounded border bg-background p-1"
                value={task[key].join("\n")}
                onChange={(e) =>
                  setTask({
                    ...task,
                    [key]: e.target.value.split("\n").filter(Boolean),
                  })
                }
              />
            </label>
          ))}
          <label>
            {t("memory")}
            <select
              aria-label={t("memoryScope")}
              className="ml-2 rounded border bg-background p-1"
              value={memoryScope}
              onChange={(e) => {
                const scope = e.target.value as typeof memoryScope
                setMemoryScope(scope)
                setMemoryText(
                  memory.find(
                    (m) => m.scope === scope && m.name === "preferences"
                  )?.content ?? ""
                )
              }}
            >
              <option value="task">{t("task")}</option>
              <option value="project">{t("project")}</option>
              <option value="global">{t("global")}</option>
            </select>
            <textarea
              rows={2}
              className="mt-1 w-full rounded border bg-background p-1"
              value={memoryText}
              onChange={(e) => setMemoryText(e.target.value)}
            />
          </label>
          <p className="text-muted-foreground">{t("loadRule")}</p>
          <button
            type="button"
            disabled={saving}
            className="justify-self-end rounded border px-3 py-1"
            onClick={() => {
              void save()
            }}
          >
            {t("save")}
          </button>
        </div>
      )}
    </div>
  )
}
