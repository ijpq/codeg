import { act, render } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import {
  MessageBottomFollowGuard,
  pinViewportToExactBottom,
} from "./message-bottom-follow-guard"

const h = vi.hoisted(() => ({
  viewport: null as HTMLElement | null,
  content: null as HTMLElement | null,
  state: { escapedFromLock: false },
  scrollToBottom: vi.fn(async () => true),
  observers: [] as Array<{
    callback: ResizeObserverCallback
    observe: ReturnType<typeof vi.fn>
    disconnect: ReturnType<typeof vi.fn>
  }>,
}))

vi.mock("use-stick-to-bottom", () => ({
  useStickToBottomContext: () => ({
    scrollRef: { current: h.viewport },
    contentRef: { current: h.content },
    scrollToBottom: h.scrollToBottom,
    state: h.state,
  }),
}))

class ResizeObserverStub {
  readonly callback: ResizeObserverCallback
  readonly observe = vi.fn()
  readonly disconnect = vi.fn()

  constructor(callback: ResizeObserverCallback) {
    this.callback = callback
    h.observers.push(this)
  }
}

function setGeometry(
  element: HTMLElement,
  geometry: { scrollHeight: number; clientHeight: number; scrollTop: number }
) {
  Object.defineProperties(element, {
    scrollHeight: { configurable: true, value: geometry.scrollHeight },
    clientHeight: { configurable: true, value: geometry.clientHeight },
    scrollTop: {
      configurable: true,
      writable: true,
      value: geometry.scrollTop,
    },
  })
}

async function flushTwoFrames() {
  await act(async () => {
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
    )
  })
}

beforeEach(() => {
  h.viewport = document.createElement("div")
  h.content = document.createElement("div")
  h.state.escapedFromLock = false
  h.scrollToBottom.mockClear()
  h.observers.length = 0
  vi.stubGlobal("ResizeObserver", ResizeObserverStub)
})

describe("pinViewportToExactBottom", () => {
  it("eliminates a fractional/rounded final-line gap", () => {
    const viewport = document.createElement("div")
    setGeometry(viewport, {
      scrollHeight: 700.75,
      clientHeight: 300.25,
      scrollTop: 399,
    })

    expect(pinViewportToExactBottom(viewport)).toBe(true)
    expect(viewport.scrollTop).toBe(400.5)
  })

  it("does not rewrite an already exact viewport", () => {
    const viewport = document.createElement("div")
    setGeometry(viewport, {
      scrollHeight: 700,
      clientHeight: 300,
      scrollTop: 400,
    })

    expect(pinViewportToExactBottom(viewport)).toBe(false)
  })
})

describe("MessageBottomFollowGuard", () => {
  it("re-pins when the composer changes the viewport height", async () => {
    setGeometry(h.viewport!, {
      scrollHeight: 1000,
      clientHeight: 400,
      scrollTop: 600,
    })
    const view = render(<MessageBottomFollowGuard layoutSignal="running:1" />)
    await flushTwoFrames()

    setGeometry(h.viewport!, {
      scrollHeight: 1000,
      clientHeight: 360,
      scrollTop: 600,
    })
    act(() => {
      h.observers[0]!.callback([], h.observers[0] as unknown as ResizeObserver)
    })
    await flushTwoFrames()

    expect(h.viewport!.scrollTop).toBe(640)
    expect(h.scrollToBottom).toHaveBeenCalled()
    view.unmount()
    expect(h.observers[0]!.disconnect).toHaveBeenCalledTimes(1)
  })

  it("never pulls the reader back after an intentional upward scroll", async () => {
    setGeometry(h.viewport!, {
      scrollHeight: 1000,
      clientHeight: 400,
      scrollTop: 250,
    })
    h.state.escapedFromLock = true
    render(<MessageBottomFollowGuard layoutSignal="complete:2" />)

    act(() => {
      h.observers[0]!.callback([], h.observers[0] as unknown as ResizeObserver)
    })
    await flushTwoFrames()

    expect(h.viewport!.scrollTop).toBe(250)
    expect(h.scrollToBottom).not.toHaveBeenCalled()
  })
})
