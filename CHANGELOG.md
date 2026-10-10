# Changelog

## 0.34.0-fix1 — 2026-10-10

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

See [release notes](docs/releases/v0.34.0-fix1.md) for scope, validation and limits.
