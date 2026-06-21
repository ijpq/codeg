"use client"

import type { ReactNode } from "react"
import { useTranslations } from "next-intl"
import { Loader2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { cn } from "@/lib/utils"
import type { AgentOptionsSnapshot, SessionConfigOptionInfo } from "@/lib/types"

// Picking this clears the override (inherit the agent's own default). Mirrors
// delegation-agent-defaults.tsx; the codeg prefix avoids colliding with a real
// option id.
const DEFAULT_SENTINEL = "__codeg_default__"

interface AgentConfigSectionProps {
  /** Probe result, owned by the parent (so a single probe also feeds the `/`
   *  command menu). Null while loading / on error / before the first probe. */
  snapshot: AgentOptionsSnapshot | null
  loading: boolean
  error: string | null
  onReload: () => void
  modeId: string | null
  configValues: Record<string, string>
  onModeChange: (modeId: string | null) => void
  onConfigChange: (optionId: string, valueId: string | null) => void
  /** "stacked" (default) renders the labeled card used in standalone forms;
   *  "inline" renders compact label-less select chips that sit in the
   *  composer-style editor's bottom bar. */
  layout?: "stacked" | "inline"
}

/**
 * The composer's model / mode / permission config surface. The probe is owned
 * by the parent (`useAgentOptions`) and passed in, so the editor runs a single
 * transient session that feeds both these selectors and the `/` command menu.
 * The model is one of the config options (id/category "model"); no special-casing.
 */
export function AgentConfigSection({
  snapshot,
  loading,
  error,
  onReload,
  modeId,
  configValues,
  onModeChange,
  onConfigChange,
  layout = "stacked",
}: AgentConfigSectionProps) {
  const t = useTranslations("Automations")
  const inline = layout === "inline"

  if (loading) {
    return (
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
        {t("probing")}
      </div>
    )
  }
  if (error) {
    return (
      <div className="flex flex-col items-start gap-2">
        <p className="text-xs text-destructive">{error}</p>
        <Button size="sm" variant="outline" onClick={onReload}>
          {t("retry")}
        </Button>
      </div>
    )
  }
  if (!snapshot) return null

  const hasModes = !!snapshot.modes && snapshot.modes.available_modes.length > 0
  const hasOptions = snapshot.config_options.length > 0
  if (!hasModes && !hasOptions) {
    // Inline lives in the composer bottom bar — stay silent rather than print a
    // sentence there; the stacked form still surfaces the hint.
    if (inline) return null
    return <p className="text-xs text-muted-foreground">{t("configNone")}</p>
  }
  // Mirror the composer: when an agent exposes both modes AND config options,
  // hide the standalone mode row (some agents surface mode as a config option).
  const showMode = hasModes && !hasOptions

  return (
    <div
      className={cn(
        inline
          ? "flex flex-wrap items-center gap-x-3 gap-y-1.5"
          : "flex flex-col gap-2.5 rounded-lg border border-border bg-card/40 p-3"
      )}
    >
      {showMode && snapshot.modes ? (
        <FlatSelect
          label={t("mode")}
          value={modeId}
          inheritLabel={t("inherit")}
          inline={inline}
          allowInherit={!inline}
          currentValue={snapshot.modes.current_mode_id}
          onChange={onModeChange}
          items={snapshot.modes.available_modes.map((m) => ({
            value: m.id,
            name: m.name,
          }))}
        />
      ) : null}
      {snapshot.config_options.map((option) => (
        <ConfigOptionRow
          key={option.id}
          option={option}
          value={configValues[option.id] ?? null}
          inheritLabel={t("inherit")}
          inline={inline}
          allowInherit={!inline}
          onChange={(v) => onConfigChange(option.id, v)}
        />
      ))}
    </div>
  )
}

// The shared row shell (label + Select trigger) for both the standalone mode
// row and the per-option rows. Keeping the inline-vs-stacked styling here means
// the mode chip and the config chips can never drift apart in the composer's
// bottom bar; callers supply only the differing <SelectContent>.
function FieldRow({
  label,
  value,
  inline,
  allowInherit,
  currentValue,
  onChange,
  children,
}: {
  label: string
  value: string | null
  inline?: boolean
  /** When false (automations), the "inherit/default" escape hatch is dropped:
   *  the selector pins a concrete value, defaulting to the agent's *current*
   *  value so the shown choice always matches what an unset option would run. */
  allowInherit: boolean
  currentValue?: string | null
  onChange: (v: string | null) => void
  children: ReactNode
}) {
  const selectValue = allowInherit
    ? (value ?? DEFAULT_SENTINEL)
    : (value ?? currentValue ?? "")
  return (
    <div
      className={
        inline
          ? "flex items-center gap-1.5"
          : "flex items-center justify-between gap-3"
      }
    >
      {/* Inline (composer bottom bar) drops the visible label entirely — the
          chip shows only its value, like the composer's model/mode selectors. */}
      {!inline ? (
        <label className="min-w-0 truncate text-sm">{label}</label>
      ) : null}
      <Select
        value={selectValue}
        onValueChange={(v) =>
          onChange(allowInherit ? (v === DEFAULT_SENTINEL ? null : v) : v)
        }
      >
        <SelectTrigger
          size="sm"
          // The dropped label still rides along for hover/screen readers.
          aria-label={label}
          title={inline ? label : undefined}
          className={
            inline
              ? "h-7 w-auto max-w-[12rem] gap-1 border-0 bg-transparent px-1.5 text-xs text-muted-foreground shadow-none hover:text-foreground"
              : "w-52"
          }
        >
          <SelectValue />
        </SelectTrigger>
        {children}
      </Select>
    </div>
  )
}

function FlatSelect({
  label,
  value,
  inheritLabel,
  inline,
  allowInherit,
  currentValue,
  onChange,
  items,
}: {
  label: string
  value: string | null
  inheritLabel: string
  inline?: boolean
  allowInherit: boolean
  currentValue?: string | null
  onChange: (v: string | null) => void
  items: Array<{ value: string; name: string }>
}) {
  return (
    <FieldRow
      label={label}
      value={value}
      inline={inline}
      allowInherit={allowInherit}
      currentValue={currentValue}
      onChange={onChange}
    >
      <SelectContent>
        {allowInherit ? (
          <SelectItem value={DEFAULT_SENTINEL}>{inheritLabel}</SelectItem>
        ) : null}
        {items.map((it) => (
          <SelectItem key={it.value} value={it.value}>
            {it.name}
          </SelectItem>
        ))}
      </SelectContent>
    </FieldRow>
  )
}

function ConfigOptionRow({
  option,
  value,
  inheritLabel,
  inline,
  allowInherit,
  onChange,
}: {
  option: SessionConfigOptionInfo
  value: string | null
  inheritLabel: string
  inline?: boolean
  allowInherit: boolean
  onChange: (v: string | null) => void
}) {
  if (option.kind.type !== "select") return null
  const groups = option.kind.groups
  return (
    <FieldRow
      label={option.name}
      value={value}
      inline={inline}
      allowInherit={allowInherit}
      currentValue={option.kind.current_value}
      onChange={onChange}
    >
      <SelectContent>
        {allowInherit ? (
          <SelectItem value={DEFAULT_SENTINEL}>{inheritLabel}</SelectItem>
        ) : null}
        {groups.length > 0
          ? groups.map((g) => (
              <SelectGroup key={g.group}>
                <SelectLabel>{g.name}</SelectLabel>
                {g.options.map((it) => (
                  <SelectItem key={`${g.group}-${it.value}`} value={it.value}>
                    {it.name}
                  </SelectItem>
                ))}
              </SelectGroup>
            ))
          : option.kind.options.map((it) => (
              <SelectItem key={it.value} value={it.value}>
                {it.name}
              </SelectItem>
            ))}
      </SelectContent>
    </FieldRow>
  )
}
