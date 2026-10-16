import React, { useEffect, useState } from "react"
import { createRoot } from "react-dom/client"
import { NextIntlClientProvider } from "next-intl"
import { MessageThread } from "../../src/components/ai-elements/message-thread"
import { VirtualizedMessageThread } from "../../src/components/message/virtualized-message-thread"
import { MessageBottomFollowGuard } from "../../src/components/message/message-bottom-follow-guard"
import messages from "../../src/i18n/messages/en.json"
import "../../src/app/globals.css"

declare global {
  interface Window {
    scrollFixture: {
      change: (state: Partial<State>) => void
      writes: Array<{ top: number; stack: string }>
    }
  }
}
type State = {
  count: number
  lines: number
  image: boolean
  artifact: boolean
  dock: number
  scale: number
  smooth: boolean
}
function Fixture() {
  const [state, set] = useState<State>({
    count: 2000,
    lines: 2,
    image: false,
    artifact: false,
    dock: 100,
    scale: 1,
    smooth: false,
  })
  useEffect(() => {
    const writes: Array<{ top: number; stack: string }> = []
    window.scrollFixture = {
      change: (next) => set((current) => ({ ...current, ...next })),
      writes,
    }
    const proto = Element.prototype
    const original = Object.getOwnPropertyDescriptor(proto, "scrollTop")!
    Object.defineProperty(proto, "scrollTop", {
      ...original,
      set(value: number) {
        if ((this as Element).classList.contains("chat-scroll-port")) {
          writes.push({
            top: value,
            stack: new Error().stack?.split("\n").slice(1, 5).join("\n") ?? "",
          })
          if (writes.length > 500) writes.shift()
        }
        original.set!.call(this, value)
      },
    })
    return () => {
      Object.defineProperty(proto, "scrollTop", original)
    }
  }, [])
  const items = Array.from({ length: state.count }, (_, i) => i)
  return (
    <NextIntlClientProvider locale="en" messages={messages}>
      <div
        data-conversation-shell
        style={{
          height: "100vh",
          display: "flex",
          flexDirection: "column",
          fontSize: `${16 * state.scale}px`,
        }}
      >
        <MessageThread
          className="min-h-0 flex-1"
          resize={state.smooth ? "smooth" : "instant"}
        >
          <MessageBottomFollowGuard
            scopeKey="browser-fixture"
            layoutSignal={JSON.stringify(state)}
          />
          <VirtualizedMessageThread
            items={items}
            getItemKey={(i) => `${i}`}
            itemSize={90}
            renderItem={(i) => (
              <div style={{ border: "0.5px solid transparent" }}>
                {i === items.length - 1 ? (
                  <>
                    {Array.from({ length: state.lines }, (_, n) => (
                      <p
                        key={n}
                        style={{ lineHeight: 1.53, margin: "0.37em 0" }}
                      >
                        Long streamed response line {n + 1}. A measured virtual
                        row, code and prose.
                      </p>
                    ))}
                    {state.image && (
                      <img
                        alt="late image"
                        src="/late-image.svg"
                        style={{ width: "100%", height: "auto" }}
                      />
                    )}
                    {state.artifact && (
                      <iframe
                        title="late artifact"
                        srcDoc={
                          '<body style="margin:0;height:511.3px">Artifact loaded</body>'
                        }
                        style={{ height: 511.3, width: "100%" }}
                      />
                    )}
                    <p data-final-line>FINAL LINE MUST REMAIN VISIBLE</p>
                  </>
                ) : (
                  <p style={{ height: 57.3, margin: 0 }}>History row {i}</p>
                )}
              </div>
            )}
          />
        </MessageThread>
        <div
          data-conversation-bottom-dock
          style={{
            height: state.dock,
            flexShrink: 0,
            borderTop: "1px solid gray",
          }}
        >
          Composer / goal / artifact controls
        </div>
      </div>
    </NextIntlClientProvider>
  )
}
createRoot(document.getElementById("root")!).render(<Fixture />)
