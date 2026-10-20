# Changelog

## 0.38.0-fix1 — 2026-10-20

- Restore ordinary conversation completion labels to “Worked for …” using
  Codex's native whole-task duration, matched by `turn_id`. Count it once across
  tool/model fragments and separate consecutive automated tasks and notices.
- Continue bounded metadata reconciliation when usage arrives before timing,
  including the main conversation, Canvas and background viewers. Supersede
  stale requests and preserve already loaded history while patching metadata.
- Infer missing Codex timing only from matching explicit task start/end records;
  leave genuinely untimed old messages unknown instead of timing partial replies.
- Remove the previous release's unrelated To-dos duration additions and extra
  Goal elapsed panel. Preserve native Direct / Plan / Goal controls, safe Goal
  switching, transcript focus and the existing virtualized scrolling fixes.
- Rebase onto upstream v0.38.0 and retain its CLI, session settings and details.
  Include the matching CLI companion in both Windows packages and verify it.
- Preserve upgrade revalidation for unhashed web resources while retaining
  immutable hashed chunks and small 404 responses for missing static files.
- Test native timing, delayed metadata, stale requests, multi-tool and automated
  rounds; verify ordinary live/history labels on packaged Server Web and Setup,
  along with isolated installation, upgrade, resources, migrations and hashes.

See [release notes](docs/releases/v0.38.0-fix1.md) and
[timing diagnosis](docs/completed-turn-duration.md).

## 0.36.0-fix1 — 2026-10-17

- Rebase the fork onto upstream v0.36.0 while preserving existing recovery,
  bounded history, deliverables, native fork and collaboration fixes.
- Unify Codex execution into Direct / Plan first / Execute goal. Use native
  collaboration mode and Goal RPCs, with an objective, status, pause, resume
  and end controls. Leaving an active Goal awaits a confirmed pause and stops
  its current work; selecting Goal alone never starts or resumes it.
- Bind a draft's first Goal to durable conversation history before native
  work starts. Keep restored/shared sessions controllable and isolate
  read-only child transcripts from parent controls.
- Display native Goal continuations through the bounded transcript reader,
  coalescing out-of-prompt activity on visible surfaces and draining final
  records after pause/end. Preserve existing history pages and reader position.
  Back off asynchronous read failures without dropping pending output.
- Keep the latest Goal answer visible when a native pause/completion snapshot
  arrives after it, including history reloads with collapsed process cards.
- Show completed tasks' most recent execution duration on cards, list rows and
  details using stored start/review times, excluding the later acceptance wait.
  Show native Goal elapsed time in its controls and historical cards, to the
  second, including after completion and reload; never estimate missing times.
- Share the transcript scroll target, prevent virtual row compensation from
  falsely escaping the follow lock, and terminate unreachable-sentinel retries.
  Preserve virtualization, delayed-content following and deliberate reader escape.
- Retain Ctrl/Cmd+Home navigation outside editors while shielding programmatic
  measurement corrections; cover the shortcuts in real browser regressions.
- Preserve transcript keyboard focus when a turn settles, including native
  Goal continuations, so composer autofocus cannot redirect reader navigation.
- Add native CLI/ACP, browser and Windows WebView2 regressions; verify actual
  packaged Server Web and installed desktop pages plus isolated Setup upgrades.
- Inventory every packaged frontend resource and verify all compiled database
  migrations execute; download and verify both Windows packages before and after
  publication. Server includes the same-version codeg-mcp and pinned Codex runtime.
- Statically link the Windows C runtime and check ordinary/delayed PE imports in
  both packages, rejecting missing external redistributables before publication.

See [release notes](docs/releases/v0.36.0-fix1.md) for native protocol behavior
and validation details.

## 0.35.1-fix2 — 2026-10-13

- Repair restored Windows runtime caches that retain V8 build-script output but
  omit its native static library. Rebuild only V8 with the original pinned source;
  add four regressions including a real Cargo cache reproduction.
- Keep the failed fix1 tag unpublished; publish these complete changes as fix2.
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
  SQLite state passes unchanged checksum validation. Match Git Windows resource
  checkout bytes without touching patched sources; add eight regressions.
- Build complete Windows helpers using upstream checksum-pinned sandbox-enabled
  V8 artifacts, without disabling features or changing dependency versions.
- Add real CLI/ACP/CodeG stage diagnostics and protocol/context regressions;
  require the pinned runtime in independent Windows release verification.
- Normalize fixture paths before JSON encoding in model-context comparisons,
  retaining full assertions across Windows/Linux and adding four regressions.
- Preserve bounded recent-history paging, lifecycle generations, nonblocking AIR,
  native forks, MCP and other existing custom fixes across the upstream rebase.

See [release notes](docs/releases/v0.35.1-fix2.md) and
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
