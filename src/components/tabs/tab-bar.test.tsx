import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import type { ReactNode } from "react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"

const mocks = vi.hoisted(() => {
  const action = () => vi.fn()
  return {
    closeAllTabs: vi.fn(),
    tabState: {
      tabs: [] as Array<{
        id: string
        kind: "conversation"
        folderId: number
        conversationId: number
        agentType: "codex"
        title: string
        isPinned: boolean
      }>,
      activeTabId: null as string | null,
      groupOf: {} as Record<string, string>,
      groupLayout: { type: "group" as const, id: "g-main" },
      groupSelection: {} as Record<string, string>,
      tileByGroup: {} as Record<string, boolean>,
      tabDrag: null,
    },
    actions: {
      switchTab: action(),
      closeTab: action(),
      closeOtherTabs: action(),
      pinTab: action(),
      toggleGroupTile: action(),
      splitTab: action(),
      moveTabToGroup: action(),
      toggleGroupOrientation: action(),
      dissolveGroup: action(),
      unsplitAll: action(),
      reorderTabs: action(),
      reorderGroupTabs: action(),
      updateTabDrag: action(),
      endTabDrag: action(),
      openNewConversationTab: action(),
      openChatModeTab: action(),
    },
  }
})

vi.mock("motion/react", () => ({
  Reorder: {
    Group: ({ children, ...props }: { children: ReactNode }) => {
      const {
        values: _values,
        onReorder: _onReorder,
        ...domProps
      } = props as {
        values?: unknown
        onReorder?: unknown
      }
      void _values
      void _onReorder
      return <div {...domProps}>{children}</div>
    },
  },
}))

vi.mock("./tab-item", () => ({
  TabItem: ({ tab }: { tab: { id: string; title: string } }) => (
    <div data-tab-id={tab.id}>{tab.title}</div>
  ),
}))

vi.mock("@/contexts/tab-context", () => ({
  useTabStore: (selector: (state: typeof mocks.tabState) => unknown) =>
    selector(mocks.tabState),
  useTabActions: () => ({
    ...mocks.actions,
    closeAllTabs: mocks.closeAllTabs,
  }),
}))

vi.mock("@/stores/tab-store", () => ({
  groupOfTab: (
    groupOf: Record<string, string>,
    layout: {
      type: "group" | "split"
      id: string
      children?: Array<{ id: string }>
    },
    tabId: string
  ) =>
    groupOf[tabId] ??
    (layout.type === "group" ? layout.id : layout.children![0].id),
}))

vi.mock("@/stores/app-workspace-store", () => ({
  useAppWorkspaceStore: (
    selector: (state: {
      allFolders: Array<{ id: number; name: string; path: string }>
      branches: Map<number, string>
    }) => unknown
  ) =>
    selector({
      allFolders: [{ id: 1, name: "Repo", path: "/repo" }],
      branches: new Map(),
    }),
}))

vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({
    activeFolder: { id: 1, name: "Repo", path: "/repo" },
  }),
}))

vi.mock("@/contexts/workbench-route-context", () => ({
  useWorkbenchRoute: () => ({ openConversations: vi.fn() }),
}))

vi.mock("@/hooks/use-is-coarse-pointer", () => ({
  useIsCoarsePointer: () => false,
}))

import { TabBar } from "./tab-bar"

function tab(id: string, conversationId: number) {
  return {
    id,
    kind: "conversation" as const,
    folderId: 1,
    conversationId,
    agentType: "codex" as const,
    title: `Conversation ${conversationId}`,
    isPinned: true,
  }
}

function renderTabBar(groupId?: string) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <TabBar groupId={groupId} />
    </NextIntlClientProvider>
  )
}

describe("TabBar close-all control", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    Object.assign(mocks.tabState, {
      tabs: [tab("one", 1), tab("two", 2)],
      activeTabId: "one",
      groupOf: {},
      groupLayout: { type: "group" as const, id: "g-main" },
      groupSelection: { "g-main": "one" },
      tileByGroup: {},
      tabDrag: null,
    })
  })

  it("pins a one-click Close All button to the right of the drag spacer", () => {
    renderTabBar()

    const closeAll = screen.getByRole("button", { name: "Close All" })
    expect(closeAll.previousElementSibling).toHaveAttribute(
      "data-tauri-drag-region"
    )

    fireEvent.click(closeAll)
    expect(mocks.closeAllTabs).toHaveBeenCalledTimes(1)
  })

  it("shows only one global control in the focused split group", () => {
    Object.assign(mocks.tabState, {
      groupLayout: {
        type: "split" as const,
        id: "split",
        orientation: "horizontal" as const,
        children: [
          { type: "group" as const, id: "left" },
          { type: "group" as const, id: "right" },
        ],
        ratios: [0.5, 0.5],
      },
      groupOf: { one: "left", two: "right" },
      groupSelection: { left: "one", right: "two" },
    })

    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TabBar groupId="left" />
        <TabBar groupId="right" />
      </NextIntlClientProvider>
    )

    expect(screen.getAllByRole("button", { name: "Close All" })).toHaveLength(1)
  })
})
