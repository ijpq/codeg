import { act, render } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import {
  MessageBottomFollowGuard,
  measureMessageBottomGeometry,
  pinViewportToExactBottom,
} from "./message-bottom-follow-guard"

const h = vi.hoisted(() => ({
  viewport: null as HTMLElement | null,
  content: null as HTMLElement | null,
  state: { escapedFromLock: false },
  observers: [] as Array<{
    callback: ResizeObserverCallback
    observe: ReturnType<typeof vi.fn>
    unobserve: ReturnType<typeof vi.fn>
    disconnect: ReturnType<typeof vi.fn>
  }>,
}))

vi.mock("use-stick-to-bottom", () => ({
  useStickToBottomContext: () => ({
    scrollRef: { current: h.viewport },
    contentRef: { current: h.content },
    state: h.state,
  }),
}))

class ResizeObserverStub {
  readonly callback: ResizeObserverCallback
  readonly observe = vi.fn()
  readonly unobserve = vi.fn()
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

function setRect(
  element: HTMLElement,
  rect: Partial<DOMRect> & { top: number; bottom: number }
) {
  element.getBoundingClientRect = vi.fn(
    () =>
      ({
        x: 0,
        y: rect.top,
        left: 0,
        right: 800,
        width: 800,
        height: rect.height ?? rect.bottom - rect.top,
        toJSON: () => ({}),
        ...rect,
      }) as DOMRect
  )
}

async function flushFrames(count = 4) {
  await act(async () => {
    for (let index = 0; index < count; index += 1) {
      await new Promise<void>((resolve) =>
        requestAnimationFrame(() => resolve())
      )
    }
  })
}

beforeEach(() => {
  h.viewport = document.createElement("div")
  h.content = document.createElement("div")
  const row = document.createElement("div")
  row.dataset.messageThreadLast = "true"
  const sentinel = document.createElement("div")
  sentinel.dataset.messageEndSentinel = "true"
  sentinel.style.marginTop = "0px"
  h.content.append(row, sentinel)
  h.viewport.append(h.content)
  setRect(h.viewport, { top: 0, bottom: 400 })
  setRect(row, { top: 300, bottom: 375 })
  setRect(sentinel, { top: 375, bottom: 376, height: 1 })
  h.state.escapedFromLock = false
  h.observers.length = 0
  vi.stubGlobal("ResizeObserver", ResizeObserverStub)
})

describe("measureMessageBottomGeometry", () => {
  it("derives virtual-row overflow from the real final row", () => {
    const viewport = document.createElement("div")
    const content = document.createElement("div")
    const row = document.createElement("div")
    const sentinel = document.createElement("div")
    sentinel.style.marginTop = "8px"
    content.append(row, sentinel)
    setRect(viewport, { top: 0, bottom: 500 })
    setRect(row, { top: 300, bottom: 430 })
    setRect(sentinel, { top: 408, bottom: 409, height: 1 })

    const measured = measureMessageBottomGeometry(
      viewport,
      content,
      sentinel,
      row,
      null
    )

    // Raw virtual end = 408 - existing 8px correction = 400.
    expect(measured.virtualOverflowCorrection).toBe(30)
  })

  it("uses actual dock overlap instead of its full configured height", () => {
    const viewport = document.createElement("div")
    const content = document.createElement("div")
    const sentinel = document.createElement("div")
    const dock = document.createElement("div")
    content.append(sentinel)
    setRect(viewport, { top: 100, bottom: 600 })
    setRect(sentinel, { top: 500, bottom: 501, height: 1 })
    setRect(dock, { top: 550, bottom: 750, height: 200 })

    const measured = measureMessageBottomGeometry(
      viewport,
      content,
      sentinel,
      null,
      dock
    )

    expect(measured.dockHeight).toBe(200)
    expect(measured.dockOverlap).toBe(50)
  })

  it("keeps the measured correction while the last row is virtualized out", () => {
    const viewport = document.createElement("div")
    const content = document.createElement("div")
    const sentinel = document.createElement("div")
    sentinel.style.marginTop = "24px"
    content.append(sentinel)
    setRect(viewport, { top: 0, bottom: 500 })
    setRect(sentinel, { top: 420, bottom: 421, height: 1 })

    expect(
      measureMessageBottomGeometry(viewport, content, sentinel, null, null)
        .virtualOverflowCorrection
    ).toBe(24)
  })
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
    await flushFrames()

    setGeometry(h.viewport!, {
      scrollHeight: 1000,
      clientHeight: 360,
      scrollTop: 600,
    })
    act(() => {
      h.observers[0]!.callback([], h.observers[0] as unknown as ResizeObserver)
    })
    await flushFrames()

    expect(h.viewport!.scrollTop).toBe(640)
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
    await flushFrames()

    expect(h.viewport!.scrollTop).toBe(250)
  })
})
