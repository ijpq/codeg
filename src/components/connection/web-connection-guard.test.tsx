import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Controllable stand-in for the connection store. `vi.hoisted` guarantees the
// shared object exists before the hoisted `vi.mock` factory runs.
const store = vi.hoisted(() => {
  let state: "connected" | "reconnecting" | "unauthorized" = "connected"
  const listeners = new Set<() => void>()
  return {
    getState: () => state,
    setState: (s: "connected" | "reconnecting" | "unauthorized") => {
      state = s
      for (const l of listeners) l()
    },
    subscribe: (cb: () => void) => {
      listeners.add(cb)
      return () => {
        listeners.delete(cb)
      }
    },
    reset: () => {
      state = "connected"
      listeners.clear()
    },
    reconnectWebNow: vi.fn(),
    verifyWebConnectionNow: vi.fn(),
    redirectToCodegLogin: vi.fn(),
  }
})

vi.mock("@/lib/transport/web-connection-store", () => ({
  subscribeWebConnection: store.subscribe,
  getWebConnectionSnapshot: store.getState,
  getWebConnectionServerSnapshot: () => "connected",
  reconnectWebNow: store.reconnectWebNow,
  verifyWebConnectionNow: store.verifyWebConnectionNow,
  notifyWebUnauthorized: vi.fn(),
}))

vi.mock("@/lib/transport/web-auth", () => ({
  redirectToCodegLogin: store.redirectToCodegLogin,
  getCodegToken: () => "tok",
}))

import { WebConnectionGuard } from "./web-connection-guard"
import enMessages from "@/i18n/messages/en.json"

function renderGuard() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <WebConnectionGuard />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  vi.useFakeTimers()
  store.reset()
  store.reconnectWebNow.mockClear()
  store.verifyWebConnectionNow.mockClear()
  store.redirectToCodegLogin.mockClear()
})

afterEach(() => {
  vi.useRealTimers()
})

describe("WebConnectionGuard", () => {
  it("renders nothing while connected", () => {
    const { container } = renderGuard()
    expect(container.firstChild).toBeNull()
    expect(screen.queryByText("Connection lost")).not.toBeInTheDocument()
  })

  it("stays hidden during the grace window, then shows the reconnecting dialog", () => {
    renderGuard()
    act(() => store.setState("reconnecting"))
    // Within the grace window — a brief blip must not flash a modal.
    expect(screen.queryByText("Connection lost")).not.toBeInTheDocument()

    act(() => {
      vi.advanceTimersByTime(4000)
    })
    expect(screen.getByText("Connection lost")).toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Reconnect now" })
    ).toBeInTheDocument()
  })

  it("fires reconnectWebNow when the reconnect button is clicked", () => {
    renderGuard()
    act(() => store.setState("reconnecting"))
    act(() => {
      vi.advanceTimersByTime(4000)
    })
    fireEvent.click(screen.getByRole("button", { name: "Reconnect now" }))
    expect(store.reconnectWebNow).toHaveBeenCalledTimes(1)
  })

  it("shows the session-expired dialog immediately (no grace) for unauthorized", () => {
    renderGuard()
    act(() => store.setState("unauthorized"))
    // No timer advance: a rejected token is surfaced at once.
    expect(screen.getByText("Session expired")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Go to login" }))
    expect(store.redirectToCodegLogin).toHaveBeenCalledTimes(1)
  })

  it("dismisses automatically once the connection recovers", () => {
    renderGuard()
    act(() => store.setState("reconnecting"))
    act(() => {
      vi.advanceTimersByTime(4000)
    })
    expect(screen.getByText("Connection lost")).toBeInTheDocument()

    act(() => store.setState("connected"))
    expect(screen.queryByText("Connection lost")).not.toBeInTheDocument()
  })

  it("verifies immediately on network restore while reconnecting", () => {
    renderGuard()
    act(() => store.setState("reconnecting"))
    store.verifyWebConnectionNow.mockClear()

    act(() => {
      window.dispatchEvent(new Event("online"))
    })
    expect(store.verifyWebConnectionNow).toHaveBeenCalledTimes(1)
  })

  it("also verifies an apparently connected socket on network events", () => {
    renderGuard()
    act(() => {
      window.dispatchEvent(new Event("online"))
    })
    expect(store.verifyWebConnectionNow).toHaveBeenCalledTimes(1)
    expect(store.reconnectWebNow).not.toHaveBeenCalled()
  })

  it("verifies the path as soon as a sleeping tab becomes visible", () => {
    renderGuard()
    const visibility = vi
      .spyOn(document, "visibilityState", "get")
      .mockReturnValue("visible")

    act(() => {
      document.dispatchEvent(new Event("visibilitychange"))
    })

    expect(store.verifyWebConnectionNow).toHaveBeenCalledTimes(1)
    visibility.mockRestore()
  })
})
