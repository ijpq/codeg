import { describe, expect, it } from "vitest"

import type { FlatFileEntry } from "@/hooks/use-file-tree"
import type {
  AcpAgentInfo,
  AgentSkillItem,
  DbConversationSummary,
  ExpertListItem,
  GitLogEntry,
} from "@/lib/types"

import {
  agentToSuggestion,
  commitToSuggestion,
  expertToSuggestion,
  fileToSuggestion,
  pathToFileUri,
  sessionToSuggestion,
  skillToSuggestion,
} from "./adapters"

describe("pathToFileUri", () => {
  it("builds a triple-slash uri for a posix path", () => {
    expect(pathToFileUri("/repo/src/app.ts")).toBe("file:///repo/src/app.ts")
  })
  it("normalizes Windows backslashes and encodes the drive segment", () => {
    expect(pathToFileUri("C:\\repo\\app.ts")).toBe("file:///C%3A/repo/app.ts")
  })
  it("percent-encodes spaces, # and ? within segments (not the separators)", () => {
    expect(pathToFileUri("/a/b c#d?e.ts")).toBe("file:///a/b%20c%23d%3Fe.ts")
  })
})

describe("fileToSuggestion", () => {
  const entry: FlatFileEntry = {
    name: "app.ts",
    relativePath: "src/app.ts",
    kind: "file",
    lowerPath: "src/app.ts",
    lowerName: "app.ts",
  }
  it("maps to a file reference with a joined file:// uri", () => {
    const item = fileToSuggestion(entry, "/repo")
    expect(item.reference).toMatchObject({
      refType: "file",
      id: "src/app.ts",
      label: "app.ts",
      uri: "file:///repo/src/app.ts",
      meta: { fileKind: "file" },
    })
    expect(item.detail).toBe("src/app.ts")
  })
  it("does not double a separator when the root has a trailing slash", () => {
    expect(fileToSuggestion(entry, "/repo/").reference.uri).toBe(
      "file:///repo/src/app.ts"
    )
  })
})

describe("agentToSuggestion", () => {
  it("maps to an agent reference with no uri", () => {
    const agent = {
      agent_type: "claude_code",
      name: "Claude Code",
      description: "Anthropic CLI",
      available: true,
    } as AcpAgentInfo
    const item = agentToSuggestion(agent)
    expect(item.reference).toMatchObject({
      refType: "agent",
      id: "claude_code",
      label: "Claude Code",
      uri: null,
      meta: { agentType: "claude_code", available: true },
    })
  })
})

describe("sessionToSuggestion", () => {
  const base = {
    id: 123,
    agent_type: "codex",
    status: "in_progress",
    git_branch: "main",
  } as DbConversationSummary
  it("maps to a session reference with a codeg uri", () => {
    const item = sessionToSuggestion({ ...base, title: "Login refactor" })
    expect(item.reference).toMatchObject({
      refType: "session",
      id: "123",
      label: "Login refactor",
      uri: "codeg://session/123",
      meta: { agentType: "codex", status: "in_progress", branch: "main" },
    })
  })
  it("falls back to #id when the title is empty", () => {
    expect(sessionToSuggestion({ ...base, title: null }).reference.label).toBe(
      "#123"
    )
  })
})

describe("commitToSuggestion", () => {
  it("maps to a commit reference with an encoded repo key", () => {
    const entry = {
      hash: "abc1234",
      full_hash: "abc1234def5678",
      author: "Jane",
      date: "2026-06-10",
      message: "fix login",
      files: [],
      pushed: true,
    } as GitLogEntry
    const item = commitToSuggestion(entry, "/repo with space")
    expect(item.reference).toMatchObject({
      refType: "commit",
      id: "abc1234def5678",
      label: "abc1234",
      uri: "codeg://commit/%2Frepo%20with%20space@abc1234def5678",
      meta: { shortHash: "abc1234", message: "fix login", pushed: true },
    })
  })
})

describe("skillToSuggestion", () => {
  it("maps a user/project skill to a skill reference", () => {
    const skill = {
      id: "code-review",
      name: "Code Review",
      scope: "project",
      layout: "markdown_file",
      path: "/skills/code-review.md",
      description: "Review the diff",
      read_only: false,
    } as AgentSkillItem
    expect(skillToSuggestion(skill).reference).toMatchObject({
      refType: "skill",
      id: "code-review",
      label: "Code Review",
      uri: null,
      meta: { scope: "project" },
    })
  })
})

describe("expertToSuggestion", () => {
  const expert: ExpertListItem = {
    metadata: {
      id: "deep-research",
      category: "research",
      icon: "Sparkles",
      sort_order: 1,
      display_name: { en: "Deep Research", "zh-CN": "深度研究" },
      description: { en: "Research deeply", "zh-CN": "深入研究" },
      bundled_hash: "x",
    },
    installed_centrally: true,
    user_modified: false,
    central_path: "/experts/deep-research",
  }
  it("uses the localized display name", () => {
    expect(expertToSuggestion(expert, "zh-CN").reference.label).toBe("深度研究")
  })
  it("falls back to English then id when the locale is missing", () => {
    expect(expertToSuggestion(expert, "ja").reference.label).toBe(
      "Deep Research"
    )
  })
  it("maps to a skill reference (experts invoke as /id)", () => {
    expect(expertToSuggestion(expert, "en").reference).toMatchObject({
      refType: "skill",
      id: "deep-research",
      uri: null,
      meta: { scope: "expert", category: "research", icon: "Sparkles" },
    })
  })
})
