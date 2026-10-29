import assert from "node:assert/strict"
import { fileURLToPath } from "node:url"
import { test } from "node:test"
import { ESLint } from "eslint"

const harness = fileURLToPath(
  new URL("./verify-windows-ui.mjs", import.meta.url)
)
const eslint = new ESLint()

test("ordinary CI rejects block-scoped variables reused after server restart", async () => {
  const [result] = await eslint.lintText(
    `try {
  const conversation = { folder_id: 7 }
  console.log(conversation.folder_id)
} finally {
  console.log("stopped")
}
console.log(conversation.folder_id)
`,
    { filePath: harness }
  )
  const undefinedNames = result.messages.filter((m) => m.ruleId === "no-undef")
  assert.equal(undefinedNames.length, 1)
  assert.match(undefinedNames[0].message, /'conversation' is not defined/)
})

test("the actual Windows package harness has no undefined lexical references", async () => {
  const [result] = await eslint.lintFiles([harness])
  assert.deepEqual(
    result.messages.filter((m) => m.ruleId === "no-undef"),
    []
  )
})
