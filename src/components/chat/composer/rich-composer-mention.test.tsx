import { act, render, screen, waitFor } from "@testing-library/react"
import type { JSONContent } from "@tiptap/core"
import { createRef } from "react"
import { describe, expect, it, vi } from "vitest"

import { RichComposer, type RichComposerHandle } from "./rich-composer"
import type { ReferenceSearch } from "./suggestion/types"

const search: ReferenceSearch = () => [
  {
    kind: "file",
    label: "Files",
    items: [
      {
        reference: {
          refType: "file",
          id: "src/app.ts",
          label: "app.ts",
          uri: "file:///repo/src/app.ts",
          meta: { fileKind: "file" },
        },
        detail: "src/app.ts",
      },
    ],
  },
]

function findReference(doc: JSONContent): JSONContent | undefined {
  if (doc.type === "reference") return doc
  for (const child of doc.content ?? []) {
    const found = findReference(child)
    if (found) return found
  }
  return undefined
}

async function mount(onSubmit?: () => void) {
  const ref = createRef<RichComposerHandle>()
  render(
    <RichComposer ref={ref} referenceSearch={search} onSubmit={onSubmit} />
  )
  await waitFor(() => expect(ref.current?.getEditor()).not.toBeNull(), {
    timeout: 5000,
  })
  const editor = ref.current?.getEditor()
  if (!editor) throw new Error("editor not mounted")
  return { ref, editor }
}

describe("RichComposer @ mention integration", () => {
  it("opens the panel on @ and inserts the chosen reference", async () => {
    const { editor } = await mount()
    act(() => {
      editor.commands.insertContent("@app")
    })
    const row = await screen.findByText("app.ts", {}, { timeout: 5000 })
    act(() => {
      row.dispatchEvent(
        new MouseEvent("mousedown", { bubbles: true, cancelable: true })
      )
    })
    await waitFor(() => {
      const node = findReference(editor.getJSON())
      expect(node?.attrs).toMatchObject({ refType: "file", id: "src/app.ts" })
    })
    // The "@app" trigger text is gone, replaced by the badge.
    expect(editor.getText()).not.toContain("@app")
  })

  it("does not submit on Enter while the panel is open", async () => {
    const onSubmit = vi.fn()
    const { editor } = await mount(onSubmit)
    act(() => {
      editor.commands.insertContent("@app")
    })
    await screen.findByText("app.ts", {}, { timeout: 5000 })
    act(() => {
      ;(editor.view.dom as HTMLElement).dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          bubbles: true,
          cancelable: true,
        })
      )
    })
    expect(onSubmit).not.toHaveBeenCalled()
  })

  it("dismisses the panel on Escape", async () => {
    const { editor } = await mount()
    act(() => {
      editor.commands.insertContent("@app")
    })
    await screen.findByText("app.ts", {}, { timeout: 5000 })
    act(() => {
      ;(editor.view.dom as HTMLElement).dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Escape",
          bubbles: true,
          cancelable: true,
        })
      )
    })
    await waitFor(() => expect(screen.queryByText("app.ts")).toBeNull())
  })

  it("dismisses the panel and restores submit when referenceSearch is removed mid-open", async () => {
    const onSubmit = vi.fn()
    const ref = createRef<RichComposerHandle>()
    const { rerender } = render(
      <RichComposer ref={ref} referenceSearch={search} onSubmit={onSubmit} />
    )
    await waitFor(() => expect(ref.current?.getEditor()).not.toBeNull(), {
      timeout: 5000,
    })
    const editor = ref.current?.getEditor()
    if (!editor) throw new Error("editor not mounted")
    act(() => {
      editor.commands.insertContent("@app")
    })
    await screen.findByText("app.ts", {}, { timeout: 5000 })

    // Disable mentions while the panel is open.
    rerender(<RichComposer ref={ref} onSubmit={onSubmit} />)
    await waitFor(() =>
      expect(screen.queryByTestId("mention-popup")).toBeNull()
    )

    // Enter now submits normally — panel + plugin state were cleared.
    act(() => {
      ;(editor.view.dom as HTMLElement).dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          bubbles: true,
          cancelable: true,
        })
      )
    })
    expect(onSubmit).toHaveBeenCalled()
  })

  it("does not open a panel when referenceSearch is not provided", async () => {
    const ref = createRef<RichComposerHandle>()
    render(<RichComposer ref={ref} />)
    await waitFor(() => expect(ref.current?.getEditor()).not.toBeNull(), {
      timeout: 5000,
    })
    const editor = ref.current?.getEditor()
    if (!editor) throw new Error("editor not mounted")
    act(() => {
      editor.commands.insertContent("@app")
    })
    // Plugin is installed but inert without referenceSearch: no popup ever.
    await new Promise((resolve) => setTimeout(resolve, 250))
    expect(screen.queryByTestId("mention-popup")).toBeNull()
  })
})
