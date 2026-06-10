"use client"

import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react"
import { type Editor } from "@tiptap/core"
import { EditorContent, useEditor } from "@tiptap/react"
import { exitSuggestion } from "@tiptap/suggestion"

import { cn } from "@/lib/utils"

import { buildComposerExtensions } from "./editor-config"
import { shouldSubmitOnEnter } from "./submit-key"
import type {
  MentionController,
  MentionRenderState,
} from "./suggestion/mention-suggestion"
import { SuggestionPopup } from "./suggestion/suggestion-popup"
import type { ReferenceSearch, SuggestionPopupHandle } from "./suggestion/types"
import type { ReferenceAttrs } from "./types"

/**
 * Imperative handle exposed to the parent (e.g. the message input that owns
 * attachments, queue and send orchestration). The parent reads/writes Markdown
 * and controls focus without re-rendering the editor.
 */
export interface RichComposerHandle {
  /** Serialize the current document to Markdown. */
  getMarkdown: () => string
  /** Replace the whole document from a Markdown string. */
  setMarkdown: (markdown: string) => void
  /** Clear the document. */
  clear: () => void
  /** Focus the editor at the end of the document. */
  focus: () => void
  /** Whether the document is empty (no text, no nodes). */
  isEmpty: () => boolean
  /** Escape hatch to the underlying editor (null until initialized). */
  getEditor: () => Editor | null
}

export interface RichComposerProps {
  /** Initial content, parsed as Markdown. Applied once on creation. */
  defaultMarkdown?: string
  placeholder?: string
  autoFocus?: boolean
  disabled?: boolean
  /** Accessible label for the editing surface. */
  ariaLabel?: string
  /** Outer wrapper className (host controls border/ring/max-height). */
  className?: string
  /** Inline style for the outer wrapper (e.g. max-height). */
  style?: CSSProperties
  /**
   * Fires on every document change with the serialized Markdown. Serialization
   * runs once per keystroke *only when a handler is attached* (the call is
   * skipped entirely otherwise). Callers that persist drafts must debounce —
   * the Phase 3 draft layer owns that.
   */
  onChange?: (markdown: string) => void
  /**
   * Submit intent: Enter without Shift, while not composing (IME-safe) and not
   * inside a code block. The host decides what "submit" means.
   */
  onSubmit?: () => void
  onFocus?: () => void
  onBlur?: () => void
  /**
   * Enables the unified `@` mention panel. Resolves the typed query into
   * grouped suggestions (files/agents/sessions/commits/skills). MUST be
   * referentially stable (memoize it) — it is a dependency of the panel's fetch
   * effect. Omit to disable mentions.
   */
  referenceSearch?: ReferenceSearch
}

/**
 * Rich-text composer: a Tiptap editor with live WYSIWYG Markdown, IME-safe
 * Enter-to-submit, inline reference badges, and an optional unified `@` mention
 * panel (enabled by `referenceSearch`). Not yet wired into message-input — that
 * integration (drafts, attachments, real data sources) is Phase 3.
 */
export const RichComposer = forwardRef<RichComposerHandle, RichComposerProps>(
  function RichComposer(
    {
      defaultMarkdown,
      placeholder,
      autoFocus,
      disabled,
      ariaLabel,
      className,
      style,
      onChange,
      onSubmit,
      onFocus,
      onBlur,
      referenceSearch,
    },
    ref
  ) {
    // Keep callbacks in refs so the editor (and its keymap) is created once and
    // never torn down just because a parent re-renders with new closures.
    const onChangeRef = useRef(onChange)
    const onSubmitRef = useRef(onSubmit)
    const onFocusRef = useRef(onFocus)
    const onBlurRef = useRef(onBlur)
    // Latest referenceSearch, read at event time so the mention plugin (always
    // installed) is gated on whether mentions are currently enabled — robust to
    // the prop being added/removed after the editor is created once.
    const referenceSearchRef = useRef(referenceSearch)
    useEffect(() => {
      onChangeRef.current = onChange
      onSubmitRef.current = onSubmit
      onFocusRef.current = onFocus
      onBlurRef.current = onBlur
      referenceSearchRef.current = referenceSearch
    })

    // ── Unified `@` mention panel state bridge ──
    // The suggestion plugin lives in ProseMirror; its lifecycle is bridged to
    // this React state so the popup can render in-tree (where data hooks work).
    const [mentionState, setMentionState] = useState<MentionRenderState | null>(
      null
    )
    // Mirrors `mentionState != null` for synchronous reads inside handleKeyDown
    // (so Enter defers to the panel without waiting for a re-render).
    const mentionOpenRef = useRef(false)
    const popupRef = useRef<SuggestionPopupHandle>(null)
    // Stable controller created once (refs/setState are stable), so the editor
    // is built a single time with it.
    const mentionController = useMemo<MentionController>(
      () => ({
        onStart: (mention) => {
          // Inert unless mentions are enabled (no referenceSearch → no panel).
          if (!referenceSearchRef.current) return
          mentionOpenRef.current = true
          setMentionState(mention)
        },
        onUpdate: (mention) => {
          if (!referenceSearchRef.current) return
          setMentionState(mention)
        },
        onExit: () => {
          mentionOpenRef.current = false
          setMentionState(null)
        },
        onKeyDown: (event) => popupRef.current?.onKeyDown(event) ?? false,
      }),
      []
    )

    const editor = useEditor({
      // Static export / SSR safety: never render on the server.
      immediatelyRender: false,
      // The mention plugin is always installed (the editor is created once);
      // it stays inert until `referenceSearch` is set (checked at runtime in the
      // controller). `mentionController` (stable, from useMemo) captures refs
      // but only dereferences them inside event-time callbacks, never during
      // render — the React Compiler lint can't prove that. Mirrors Tiptap's own
      // React suggestion pattern (render() → component.ref.onKeyDown).
      // eslint-disable-next-line react-hooks/refs
      extensions: buildComposerExtensions({ placeholder, mentionController }),
      editable: !disabled,
      autofocus: autoFocus ? "end" : false,
      editorProps: {
        attributes: {
          class: "codeg-composer-content",
          role: "textbox",
          "aria-multiline": "true",
          ...(ariaLabel ? { "aria-label": ariaLabel } : {}),
        },
        handleKeyDown: (view, event) => {
          // Only Enter is special; let everything else fall through cheaply.
          if (event.key !== "Enter") return false
          // While the `@` panel is open it owns Enter (select / close); never
          // submit. Checked synchronously via a ref to beat the re-render.
          if (mentionOpenRef.current) return false
          // Resolve structural context: code blocks and list items keep Enter
          // (newline / list split) instead of submitting.
          const { $from } = view.state.selection
          let inCodeBlock = $from.parent.type.spec.code === true
          let inList = false
          for (let depth = $from.depth; depth > 0; depth--) {
            const name = $from.node(depth).type.name
            if (name === "codeBlock") inCodeBlock = true
            if (name === "listItem" || name === "taskItem") inList = true
          }
          const submit = shouldSubmitOnEnter(
            {
              key: event.key,
              shiftKey: event.shiftKey,
              altKey: event.altKey,
              ctrlKey: event.ctrlKey,
              metaKey: event.metaKey,
              isComposing: event.isComposing,
              keyCode: (event as { keyCode?: number }).keyCode ?? 0,
            },
            { composing: view.composing, inCodeBlock, inList }
          )
          if (submit && onSubmitRef.current) {
            onSubmitRef.current()
            return true
          }
          return false
        },
      },
      onCreate: ({ editor }) => {
        if (defaultMarkdown) {
          editor.commands.setContent(defaultMarkdown, {
            contentType: "markdown",
            emitUpdate: false,
          })
        }
      },
      onUpdate: ({ editor }) => {
        onChangeRef.current?.(editor.getMarkdown())
      },
      onFocus: () => onFocusRef.current?.(),
      onBlur: () => onBlurRef.current?.(),
    })

    // Reflect disabled changes onto the live editor. Pass emitUpdate=false so
    // toggling editability never fires onUpdate/onChange without a real edit.
    useEffect(() => {
      editor?.setEditable(!disabled, false)
    }, [editor, disabled])

    useImperativeHandle(
      ref,
      (): RichComposerHandle => ({
        getMarkdown: () => editor?.getMarkdown() ?? "",
        setMarkdown: (markdown) =>
          editor?.commands.setContent(markdown, { contentType: "markdown" }),
        clear: () => editor?.commands.clearContent(true),
        focus: () => editor?.commands.focus("end"),
        isEmpty: () => editor?.isEmpty ?? true,
        getEditor: () => editor ?? null,
      }),
      [editor]
    )

    const closeMention = useCallback(() => {
      mentionOpenRef.current = false
      setMentionState(null)
      // Also dismiss the Tiptap suggestion plugin so its state can't stay active
      // while React thinks the panel is closed (onExit will also fire).
      const view = editor?.view
      if (view) exitSuggestion(view)
    }, [editor])

    // If mentions get disabled while a panel is open, actively dismiss it so the
    // editor's Enter handling and the plugin state return to normal (the popup
    // also unmounts via the render guard below).
    useEffect(() => {
      if (!referenceSearch && mentionOpenRef.current) closeMention()
    }, [referenceSearch, closeMention])

    const handleReferenceSelect = useCallback(
      (reference: ReferenceAttrs, range: { from: number; to: number }) => {
        editor
          ?.chain()
          .focus()
          .deleteRange(range)
          .insertReference(reference)
          .insertContent(" ")
          .run()
        closeMention()
      },
      [editor, closeMention]
    )

    return (
      <div
        className={cn("codeg-composer flex min-h-0 flex-col", className)}
        style={style}
        data-disabled={disabled || undefined}
      >
        <EditorContent
          editor={editor}
          className="codeg-composer-scroll min-h-0 flex-1 overflow-y-auto px-3 py-2 text-base md:text-sm"
        />
        {referenceSearch && mentionState && (
          <SuggestionPopup
            ref={popupRef}
            state={mentionState}
            search={referenceSearch}
            onSelect={handleReferenceSelect}
            onClose={closeMention}
          />
        )}
      </div>
    )
  }
)
