import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { ExecutionModeControl } from "./execution-mode-control"
import { GoalControlProvider } from "@/components/message/goal-control-context"
import {
  acpGetGoalState,
  acpGoalControl,
  type SessionGoalState,
} from "@/lib/api"
import messages from "@/i18n/messages/en.json"
import type { SessionConfigOptionInfo } from "@/lib/types"
vi.mock("@/lib/api", () => ({
  acpGetGoalState: vi.fn(),
  acpGoalControl: vi.fn(),
}))
let state: SessionGoalState
const option = {
  id: "collaboration_mode",
  name: "Collaboration mode",
  kind: {
    type: "select",
    current_value: "default",
    options: [
      { value: "default", name: "Default" },
      { value: "plan", name: "Plan" },
    ],
    groups: [],
  },
} as SessionConfigOptionInfo
function mount(onSelect = vi.fn()) {
  render(
    <NextIntlClientProvider locale="en" messages={messages}>
      <GoalControlProvider
        value={{
          connectionId: "c1",
          readOnly: false,
          onGoalControl: null,
          actions: [],
        }}
      >
        <ExecutionModeControl option={option} onSelect={onSelect} />
      </GoalControlProvider>
    </NextIntlClientProvider>
  )
  return onSelect
}
async function open() {
  await userEvent.click(screen.getByRole("button", { name: /^Execution:/ }))
  await waitFor(() =>
    expect(screen.getByRole("radio", { name: "Execute goal" })).toBeEnabled()
  )
}
beforeEach(() => {
  vi.clearAllMocks()
  state = { goal: null, actions: ["set", "pause", "resume", "clear"] }
  vi.mocked(acpGetGoalState).mockImplementation(async () =>
    structuredClone(state)
  )
  vi.mocked(acpGoalControl).mockImplementation(
    async (_id, action, objective) => {
      if (action === "set")
        state.goal = { objective: objective!, status: "active" }
      else if (action === "clear") state.goal = null
      else if (state.goal)
        state.goal.status = action === "pause" ? "paused" : "active"
    }
  )
})
describe("ExecutionModeControl", () => {
  it("uses the native collaboration option for Plan", async () => {
    const select = mount()
    await open()
    await userEvent.click(screen.getByRole("radio", { name: "Plan first" }))
    await waitFor(() =>
      expect(select).toHaveBeenCalledWith("collaboration_mode", "plan")
    )
    expect(acpGoalControl).not.toHaveBeenCalled()
  })
  it("creates a real goal with its objective and never starts from a selection alone", async () => {
    mount()
    await open()
    await userEvent.click(screen.getByRole("radio", { name: "Execute goal" }))
    expect(acpGoalControl).not.toHaveBeenCalled()
    fireEvent.change(screen.getByRole("textbox", { name: "Goal objective" }), {
      target: { value: "  ship tested changes  " },
    })
    await userEvent.click(screen.getByRole("button", { name: "Start goal" }))
    await waitFor(() =>
      expect(acpGoalControl).toHaveBeenCalledWith(
        "c1",
        "set",
        "ship tested changes"
      )
    )
    expect(await screen.findByText("Status: Active")).toBeInTheDocument()
  })
  it("waits for pause confirmation before changing mode; returning to Goal requires explicit resume", async () => {
    state.goal = { objective: "ship", status: "active" }
    const select = mount()
    await open()
    let confirm!: () => void
    vi.mocked(acpGoalControl).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          confirm = () => {
            state.goal!.status = "paused"
            resolve()
          }
        })
    )
    await userEvent.click(screen.getByRole("radio", { name: "Plan first" }))
    expect(select).not.toHaveBeenCalled()
    await act(async () => confirm())
    await waitFor(() =>
      expect(select).toHaveBeenCalledWith("collaboration_mode", "plan")
    )
    await userEvent.click(screen.getByRole("radio", { name: "Execute goal" }))
    expect(acpGoalControl).toHaveBeenCalledTimes(1)
    await userEvent.click(screen.getByRole("button", { name: "Resume" }))
    await waitFor(() =>
      expect(acpGoalControl).toHaveBeenLastCalledWith("c1", "resume", undefined)
    )
    await userEvent.click(screen.getByRole("button", { name: "End goal" }))
    await waitFor(() =>
      expect(acpGoalControl).toHaveBeenLastCalledWith("c1", "clear", undefined)
    )
  })
  it("keeps the Goal selected when pause is rejected", async () => {
    state.goal = { objective: "ship", status: "active" }
    const select = mount()
    await open()
    vi.mocked(acpGoalControl).mockRejectedValueOnce(new Error("pause rejected"))
    await userEvent.click(
      screen.getByRole("radio", { name: "Execute directly" })
    )
    expect(await screen.findByRole("alert")).toHaveTextContent("pause rejected")
    expect(select).not.toHaveBeenCalled()
    expect(screen.getByRole("radio", { name: "Execute goal" })).toHaveAttribute(
      "aria-checked",
      "true"
    )
  })
  it("does not offer Goal creation for an adapter without the complete native vocabulary", async () => {
    state.actions = ["pause", "clear"]
    mount()
    await userEvent.click(screen.getByRole("button", { name: /^Execution:/ }))
    expect(
      await screen.findByRole("radio", { name: "Execute goal" })
    ).toBeDisabled()
    expect(acpGoalControl).not.toHaveBeenCalled()
  })
})

it("does not restart background polling when an inactive surface's pending read finishes", async () => {
  let finish!: (value: SessionGoalState) => void
  vi.mocked(acpGetGoalState).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      })
  )
  const fixture = (active: boolean) => (
    <NextIntlClientProvider locale="en" messages={messages}>
      <GoalControlProvider
        value={{ connectionId: "c1", onGoalControl: null, actions: [] }}
      >
        <ExecutionModeControl option={option} active={active} />
      </GoalControlProvider>
    </NextIntlClientProvider>
  )
  const view = render(fixture(true))
  await waitFor(() => expect(acpGetGoalState).toHaveBeenCalledTimes(1))
  view.rerender(fixture(false))
  await act(async () => {
    finish(state)
    await Promise.resolve()
  })
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 1600))
  })
  expect(acpGetGoalState).toHaveBeenCalledTimes(1)
  view.unmount()
})

it("keeps an observed active goal visible and disables switching when status reads fail", async () => {
  state.goal = { objective: "keep this visible", status: "active" }
  mount()
  await open()
  vi.mocked(acpGetGoalState).mockRejectedValue(new Error("offline"))
  await waitFor(
    () =>
      expect(
        screen.getByText(messages.Folder.chat.execution.stateUnavailable)
      ).toBeInTheDocument(),
    { timeout: 2500 }
  )
  expect(
    screen.getByRole("button", { name: "Execution: Execute goal" })
  ).toBeInTheDocument()
  expect(screen.getByRole("radio", { name: "Plan first" })).toBeDisabled()
  expect(screen.getByText("keep this visible")).toBeInTheDocument()
})
