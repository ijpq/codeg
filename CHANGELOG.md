# Changelog

## 0.35.1-fix1 — 2026-10-12

- Rebase the custom fork onto the latest published upstream v0.35.1 commit.
- Retain ready server connections when a sidebar preview is replaced, avoiding
  unnecessary cold Codex recovery while preserving explicit disconnect/cancel.
- Ship a pinned private Codex 0.162.1 runtime in Windows Server ZIP. Recover
  eligible legacy histories from a validated native checkpoint and its suffix;
  preserve the original session, context, tools, images and complete history.
- Revalidate and reuse history inside a single resume request after asynchronous
  configuration loading; carry revisions into the native writer lock to avoid
  duplicate full reads, including when old checkpoints require full fallback.
- Fall back safely for ambiguous runtime/settings/turn state, corrupt/oversized
  tails, missing checkpoints, append/replacement and unavailable file identity.
- Preserve official Windows SQL migration bytes during runtime builds so existing
  SQLite state passes unchanged checksum validation; add five regressions.
- Build complete Windows helpers using upstream checksum-pinned sandbox-enabled
  V8 artifacts, without disabling features or changing dependency versions.
- Add real CLI/ACP/CodeG stage diagnostics and protocol/context regressions;
  require the pinned runtime in independent Windows release verification.
- Normalize fixture paths before JSON encoding in model-context comparisons,
  retaining full assertions across Windows/Linux and adding three regressions.
- Preserve bounded recent-history paging, lifecycle generations, nonblocking AIR,
  native forks, MCP and other existing custom fixes across the upstream rebase.

See [release notes](docs/releases/v0.35.1-fix1.md) and
[recovery diagnosis](docs/codex-long-session-resume.md) for evidence and limits.

## 0.34.0-fix2 — 2026-10-10

- Resolve draft releases with the draft-aware GitHub CLI and push-scoped
  verification token; reuse existing drafts by listing releases. The fix1 tag
  remains an unpublished attempt after the draft verification gate rejected it.
- Rebase the custom fork onto the published upstream v0.34.0 commit.
- Isolate history refresh generations and prevent invalidated pending reads from
  repopulating the recent-history cache.
- Recover remote turns on former-owner clients while retaining local final replies
  until their persisted turn identity and displayed content are covered.
- Preserve complete recent records beyond an oversized JSONL record; expose partial
  pages and invalidate old partial offset indexes without rewriting rollouts.
- Keep the latest 25-round load bounded, with one backwards scan capped at 64 MiB.
- Port nonblocking AIR publication to the pinned codex-acp 2.1.1 native diff protocol.
- Verify Windows Setup and Server ZIP versions, hashes, installation and isolated
  startup before publishing, and independently download and verify them afterwards.

See [release notes](docs/releases/v0.34.0-fix2.md) for scope, validation and limits.
