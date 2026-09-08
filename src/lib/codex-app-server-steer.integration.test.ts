import {
  spawn,
  spawnSync,
  type ChildProcessWithoutNullStreams,
} from "node:child_process"
import { mkdtemp, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { createInterface } from "node:readline"

import { describe, expect, it } from "vitest"

type RpcResponse = {
  id?: number
  result?: unknown
  error?: { code?: number; message?: string }
}

function codexCommand(): string | null {
  const configured = process.env.CODEG_CODEX_APP_SERVER_BIN?.trim()
  if (configured) return configured
  const probe = spawnSync("codex", ["--version"], { encoding: "utf8" })
  return probe.status === 0 ? "codex" : null
}

function request(
  child: ChildProcessWithoutNullStreams,
  responses: Map<number, (message: RpcResponse) => void>,
  id: number,
  method: string,
  params: unknown
): Promise<RpcResponse> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      responses.delete(id)
      reject(new Error(`timed out waiting for Codex app-server ${method}`))
    }, 8_000)
    responses.set(id, (message) => {
      clearTimeout(timer)
      resolve(message)
    })
    child.stdin.write(`${JSON.stringify({ id, method, params })}\n`)
  })
}

/**
 * Real-protocol smoke test: starts the installed Codex app-server in an
 * isolated temporary Codex home and calls `turn/steer` using the exact schema
 * Codeg's adapter emits. A missing thread is intentional; the assertion is
 * that app-server recognizes the method and validates the active identifiers,
 * rather than returning JSON-RPC method-not-found.
 */
describe("Codex app-server turn/steer integration", () => {
  const command = codexCommand()
  const run = command ? it : it.skip

  run("recognizes the native method and exact request shape", async () => {
    const codexHome = await mkdtemp(join(tmpdir(), "codeg-codex-steer-"))
    const child = spawn(command!, ["app-server"], {
      cwd: codexHome,
      env: { ...process.env, CODEX_HOME: codexHome },
      stdio: ["pipe", "pipe", "pipe"],
    })
    const responses = new Map<number, (message: RpcResponse) => void>()
    const stderr: string[] = []
    child.stderr.on("data", (chunk) => stderr.push(String(chunk)))
    const lines = createInterface({ input: child.stdout })
    lines.on("line", (line) => {
      let message: RpcResponse
      try {
        message = JSON.parse(line) as RpcResponse
      } catch {
        return
      }
      if (typeof message.id !== "number") return
      const resolve = responses.get(message.id)
      if (!resolve) return
      responses.delete(message.id)
      resolve(message)
    })

    try {
      const initialized = await request(child, responses, 1, "initialize", {
        clientInfo: {
          name: "codeg-steer-integration-test",
          title: "Codeg Steer Integration Test",
          version: "1",
        },
        capabilities: null,
      })
      expect(initialized.error).toBeUndefined()
      child.stdin.write(`${JSON.stringify({ method: "initialized" })}\n`)

      const response = await request(child, responses, 2, "turn/steer", {
        threadId: "codeg-missing-thread",
        expectedTurnId: "codeg-missing-turn",
        input: [{ type: "text", text: "check B instead" }],
        clientUserMessageId: "codeg-guide-message-1",
      })

      expect(response.result).toBeUndefined()
      expect(response.error).toBeDefined()
      expect(response.error?.code).not.toBe(-32601)
      expect(response.error?.message?.toLowerCase()).not.toContain(
        "method not found"
      )
    } catch (error) {
      throw new Error(
        `${error instanceof Error ? error.message : String(error)}\n${stderr.join("")}`
      )
    } finally {
      lines.close()
      child.kill()
      await rm(codexHome, { recursive: true, force: true })
    }
  })
})
