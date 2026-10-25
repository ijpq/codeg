# Ordinary completed conversation duration

The affected UI is the existing completion header above an assistant round:
`工作了 2 分钟 48 秒`, rather than only `工作已结束`. It is not the To-dos
board, Goal accounting, model text or a new timing panel.

## Previous change audit

Original commit `e6c4f21bb076c1d3fe4fb1c9bfb949fd6c585ce5` (rebased as
`0321bae50`) added timing to To-dos cards/rows/details and an extra Goal control
panel field. Those changes misunderstood this request and are selectively
removed, including their helper components, translations and assertions.
The pre-existing Goal card formatting is restored. The same commit's valid
transcript focus guard is retained, as are subsequent asynchronous test fixes.
Native Goal/Plan execution and the virtualized scroll corrections are retained.

## Reproduced causes

1. Codex history parsing ignored native `task_complete.duration_ms` and its
   `turn_id`. Generic message timestamp tiling measured reply fragments rather
   than the complete execution, and could also time non-execution notices.
2. `syncTurnMetadata` based its retry decision on missing usage. Usage already
   present could stop reconciliation while native duration was still missing.
3. The main conversation uses a different completion path:
   `reconcileCompletedTurn` committed final content and stopped without waiting
   for timing. Fixing only the usage condition would leave this path broken.
4. A cold history read can see final content before the terminal record has
   flushed. Loading or refreshing a completed native task with missing timing
   now starts the same bounded reconciliation, without replacing its content.

Two failing regression tests captured the latter paths before implementation.
Diagnostic outputs are retained under
`artifacts/completed-turn-duration-20261020/` in the verification workspace.

## Data and synchronization

`native_task_id` is the native Codex execution `turn_id`, independent of parsed
fragment IDs and agent message/fork IDs. The parser correlates explicit start,
context and terminal records by this ID. Native milliseconds take priority:
168795, 615361 and 309047 render as 2m 48s, 10m 15s and 5m 9s.
One execution's duration appears only on its last assistant fragment; the
existing merged completion header therefore counts it exactly once.

A bounded page may begin on `turn_context` without `task_started`. Its ID is
valid for matching a native terminal duration, but its timestamp is not used
as an invented execution start. Fallback subtraction requires explicit matching
start/end records or the terminal record's native start/end clocks. Negative
spans and genuinely missing clocks remain unknown; zero is valid. Completed
contexts cannot reopen timing for branch-return notices.

Live metadata first adopts identity only within a verified, unambiguous batch.
Known task IDs must match; multiple automated tasks cannot be guessed from
fragment counts. Persisted metadata is patched by task ID without replacing
content or truncating paginated history. A newer sync cancels the earlier sync;
in-flight responses also verify the captured local turn array and active token.
Missing duration receives the finite 1.5/3/5/8/13-second schedule independently
of usage. A genuinely untimed record cannot cause infinite polling.

## Verification scope

Automated cases cover native durations, many tool/model fragments, consecutive
automation, bounded tails, trustworthy fallback, cancellation, failed native
records, untimed legacy records, usage-before-duration, superseded responses,
background completion, cold history and per-task rendering/adapter caching.
The packaged Windows verifier checks ordinary headers through Server Web and
installed Setup WebView2, including live completion, conversation switching and
reload. It also runs the retained 2,000-row scrolling regressions and native
Plan/Goal controls. All app processes, profiles, fixtures and upgrade data are
created in disposable CI directories. No user's running CodeG is controlled.
