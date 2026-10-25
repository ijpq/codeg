import assert from "node:assert/strict"

// Native agents append to their own stores while the upgraded app is exercised.
// Restoring CodeG's DB does not rewind those stores. Verify retained old content
// with opaque, bounded pages instead of assuming it is still the current tail.
export async function retainedHistory(readPage, marker, maxPages = 4) {
  const cursors = new Set()
  let beforeCursor
  for (let pageNumber = 1; pageNumber <= maxPages; pageNumber++) {
    const detail = await readPage({ userTurnLimit: 25, beforeCursor })
    const match = detail.turns?.find(
      (turn) =>
        !turn.is_user && JSON.stringify(turn.blocks ?? []).includes(marker)
    )
    if (match) return { pages: pageNumber, turnId: match.id }
    const history = detail.history_page
    if (!history?.has_more) break
    const cursor = history.next_cursor
    assert.ok(cursor && !cursors.has(cursor), "History cursor did not advance")
    cursors.add(cursor)
    beforeCursor = cursor
  }
  assert.fail(`Pre-upgrade content missing within ${maxPages} bounded pages`)
}

export function scopedPageErrors(page) {
  const errors = []
  const details = []
  const listener = (error) => {
    errors.push(String(error))
    details.push({
      message: String(error),
      stack: error.stack,
      url: page.url?.(),
    })
  }
  page.on("pageerror", listener)
  return {
    errors,
    details,
    stop() {
      page.off("pageerror", listener)
      return [...errors]
    },
  }
}
