# Changelog

## 0.41.3-fix7 — 2026-10-31

- Complete connection cleanup through the captured Tokio runtime when the driver thread exits `block_on`. If the manager map was contended, the old ambient `tokio::spawn` panicked outside a runtime, leaving an already-exited connection registered and native Pi fork waiting until snapshot fallback. Setup and Server share the fix; the real writer-exit check remains mandatory.
- Add deterministic cleanup regressions for contention after the driver's runtime scope, independent Pi/Codex connection identity and the synchronous uncontended path. Retain the exact-source before/after reproducer; the downloaded fix6 package supplied the original panic evidence.
- Keep the bounded long-history handoff changes and add ten-second settled-page sampling to the final-package acceptance itself: no duplicate message/reply or missing duration may persist in its final observation window. Include native fork fallback reasons in verification failures without accepting a snapshot as native.
- Quarantine fix6 after its postpublication native-fork failure; retain its tag, packages and diagnostics. Rebuild and verify new Windows Server and Setup packages with matching MCP and the unchanged Pi ACP 0.0.34-codeg.5 / Pi 1.4.0 / Node 24.16.0 group. Preserve all existing recovery, compaction, execution-mode, business-context, provider, external-delivery, duration, artifact/Office and virtualized-scroll fixes.

See [release notes](docs/releases/v0.41.3-fix7.md).

## 0.41.3-fix6 — 2026-10-31 (quarantined; postpublication verification failed)

- Postpublication retesting caught a contended connection cleanup panic after native Pi fork, which led to snapshot fallback. The original tag/assets remain quarantined; prepublication and independent diagnostic passes do not qualify this candidate as an accepted delivery.

- Page indexed Codex history by both the requested user-turn count and the 64 MiB byte budget. Prefer complete rounds, then complete record boundaries; cursors advance through oversized rounds without loading the full conversation or dropping history. Retain native task identity and whole-task duration across byte pages.
- Stream-validate individual records larger than a page using bounded read chunks and a bounded preview, including UTF-8 beyond the preview and codepoints split across chunks. Retain the original session/path, exact byte range and SHA256; mark partial text and omit incomplete data URIs. Original native files remain untouched, and a source reference never claims the entire record was sent to the next Agent.
- Distinguish a building/rebuilding/limited index, page-budget pressure, an uncommitted record, invalid cursor, source change and malformed JSON. Preserve the specific cause through handoff error conversion. Failed preflight leaves the original Agent/session and canonical-history activation unchanged.
- Add regressions shaped like the reported 180.22 MiB first page and 64.66 MiB thirteenth page, plus giant tool/image records and fault cases. Final Windows Setup and Server acceptance switches through the real frontend using the packaged runtimes, independently checks received handoff context, then sends two uniquely identified rounds across reload.
- Carry all existing compaction, recovery, independent business sessions, provider, external-delivery, native Plan/Goal, duration, deliverable/Office and virtualized-scroll fixes. Keep Pi ACP 0.0.34-codeg.5 / Pi 1.4.0 / Node 24.16.0 and matching CodeG MCP. Release and cleanup remain gated on both complete downloaded Windows packages.

See [release notes](docs/releases/v0.41.3-fix6.md).

## 0.41.3-fix5 — 2026-10-31 (unpublished candidate)

- Resolve repeated compaction records across restored history, standalone dividers and inline reply steps. A proven terminal native lifecycle takes precedence over an earlier start; explicit running state still overrides generic tool settlement. Correlation uses event/call identities, preserving unrelated events and reply content without loading full history.
- Retain a host-owned lifecycle alias alongside the native completion ID, including existing ledger rows. Reattached clients can reconcile a restored start with terminal history even when they missed the live identity bridge, without timestamp matching or transcript edits.
- Retain atomic DOM transition evidence in final-package frontend verification. fix4 was blocked before publication when the Server page briefly retained an active card after completion; its builds and partial passes do not qualify as accepted release evidence. New before/after regressions cover stale starts, inline duplicates, native identity bridges and actual card rendering.
- Carry all Pi compaction admission, cancellation, persistence and restored-lifecycle fixes, with Pi ACP 0.0.34-codeg.5 / Pi 1.4.0 / Node 24.16.0. Preserve business sessions, recovery queues, providers, native Plan/Goal, duration, deliverables, Office review and virtualized scrolling. Both exact Windows packages require full downloaded installation, upgrade, frontend and postpublication SHA256 acceptance.

See [release notes](docs/releases/v0.41.3-fix5.md).

## 0.41.3-fix4 — 2026-10-30 (unpublished candidate)

- Keep the authoritative compaction lifecycle when a page reload or passive client adapts an unmatched historical tool as settled. An active native operation now continues to show its real summarizing phase and elapsed time, instead of falsely displaying completion or failure; terminal native outcomes still take precedence.
- The fix3 final-package frontend gate caught this remaining shared rendering defect before publication: the backend snapshot and durable history both correctly reported `in_progress`. Three new regressions fail before the fix; an additional history-adapter-to-renderer case covers cold attachment without a live subscription.
- Retain all Pi admission/cancellation/persistence fixes and the exact Pi ACP 0.0.34-codeg.5 / Pi 1.4.0 / Node 24.16.0 group, independent native sessions, reliable recovery, multi-provider settings, native Plan/Goal, duration, deliverables, Office review and virtualized scrolling. Both new Windows packages must pass the complete downloaded-package UI suite and postpublication redownload checks; fix3's successful builds do not replace this acceptance.

See [release notes](docs/releases/v0.41.3-fix4.md).

## 0.41.3-fix3 — 2026-10-30 (unpublished candidate)

- Keep one native Pi prompt alive through preflight compaction beyond the ordinary 30-second acknowledgement deadline while bounded native health checks prove execution is active. Handle current and legacy compaction events; real failures and Stop settle owned native execution before the turn ends.
- Bundle Pi ACP 0.0.34-codeg.5 with Pi 1.4.0 / Node 24.16.0. Fix the fallback that could terminate only a launcher while its native descendant continued; unconfirmed shutdown is a structured terminal error.
- Persist Pi compaction lifecycle and actual errors in an additive CodeG ledger. Capture the original business/session/connection identity before asynchronous processing; authoritative native or business history reloads retain failed/cancelled records and enrich successful native records once, without rewriting JSONL or inventing task durations. Interrupted startup state remains explicitly unknown/failed.
- Reject cyclic or ambiguous process-tree snapshots before any kill, preventing the independently reproduced unbounded traversal and protecting the current process. This does not establish the cause of fix2's Server allocation crash. Upstream license and source provenance are retained in both packages.
- Keep the unified compaction phase/elapsed/outcome card, native Plan/Goal, bounded history and virtualized scrolling, business handoff/recovery, providers, external deliveries, duration, deliverables and Office review. Final installed Setup and Server frontend acceptance now also checks failed compaction after reload and durable cancellation.
- Quarantine a published candidate automatically if final downloaded-package acceptance fails or is cancelled. A failed release retains its tag/assets and cannot count as accepted delivery; fix2 remains quarantined.
- Retry transient GitHub build-proof reads within a fixed budget and skip an unavailable historical proof instead of abandoning all other candidates. Source, original build/test provenance and runtime checksums remain mandatory; application and native protocol checks still run for the release commit.

See [release notes](docs/releases/v0.41.3-fix3.md).

## 0.41.3-fix2 — 2026-10-30 (quarantined; postpublication verification failed)

- Postpublication retesting caught a Server allocation failure in the old-data frontend restore flow. This release is quarantined as a prerelease; its original tag, packages and diagnostics are retained. Earlier prepublication passes do not make it an accepted delivery.

- Retain the Pi native compaction admission, cancellation and shared lifecycle display fixes from the unpublished fix1 candidate, including ordered MCP readiness updates for early-attached clients.
- Fix a cold-page restore race shared by Setup and Server: composer focus could restore by business id before the folder query loaded, and the later same-connection restore never filled the missing local working directory. Focus now waits for the folder and a confirmed restore fills only missing identity, keeping the live connection and strict send gate. Six Pi/Codex, Web/desktop regressions fail before the fix and pass afterward.
- Match the sidebar toggle's actual accessible name, including its configured shortcut, before testing the mobile Drawer. Wait for its responsive mount before one Escape and retain phase evidence; the toolbar must still be unobscured and reachable. The prior selector fails two timing regressions; the corrected five-case regression runs in ordinary and Windows package CI.
- Keep the exact bundled Pi ACP 0.0.34-codeg.4 / Pi 1.4.0 / Node 24.16.0 group and all existing business, recovery, provider, Plan/Goal, deliverable, Office, duration and scrolling checks. Both final Windows packages must pass downloaded installation, upgrade, frontend compaction/reply, restart, rollback and checksum gates.
- fix1 was blocked before publication by the Server narrow-layout gate. Its partial Setup evidence is retained and cannot replace this release's full two-package verification.

See [release notes](docs/releases/v0.41.3-fix2.md).

## 0.41.3-fix1 — 2026-10-30 (unpublished candidate)

- Rebase the complete custom feature set and Pi compaction lifecycle fixes onto upstream v0.41.3. Retain the private Codex Plan/Goal runtime, independent business sessions, recovery queues, multi-provider settings, duration, deliverables, Office review and virtualized scroll behavior.
- Bundle Pi ACP 0.0.34-codeg.4 with Pi 1.4.0 and Node 24.16.0. Native preflight compaction can outlive the ordinary prompt acknowledgement deadline while a bounded native health check proves it is active; real failures and Stop first settle the owned native execution.
- Show a single compaction record from the actual phase and elapsed time through completion, cancellation or failure; correlate native history without editing session files or fabricating progress.
- Keep Pi turns and durable cancellation pending until the native terminal response. Drain the cancelled compaction before closing the frontend turn, and confirm targeted process cleanup before timeout finalization; stale cancellation deadlines cannot interrupt a newer run.
- Publish launch-time MCP configuration to early-attached clients as an ordered event, so the first snapshot cannot permanently leave a completed conversation behind a stale send gate.
- Establish empty-business bindings through the actual composer in package acceptance. Keep complete runtime inventory checks, including hidden dependency files, and record application and verifier commits separately in reused diagnostic evidence.
- Keep Windows Setup and Server installation, old-data upgrade, actual-page replies, long native compaction, restart, rollback and downloaded-asset verification as release gates. The earlier fix6 candidate was not published; its diagnostic evidence does not stand in for this version’s final packages.

See [release notes](docs/releases/v0.41.3-fix1.md).

## 0.41.2-fix6 — 2026-10-29 (unpublished candidate)

- Fix Pi 1.4 preflight compaction being mistaken for a failed prompt after 30 seconds. Admission waits while native state confirms active compaction; real transport failures stop the native task before reporting their original cause.
- Handle current and legacy native compaction events with one shared status card for real stage, elapsed time, completion, cancellation and failure. Correlate successful live records with native history IDs and retain the native summary without editing session files.
- Put manual compaction through the same task queue and cancellation lifecycle; successful provider retries no longer retain an earlier transient failure.
- Bundle Pi ACP 0.0.34-codeg.4 with Pi 1.4.0 and Node 24.16.0 for Windows Server and Setup. Keep existing Plan/Goal, scrolling, business handoff/recovery, multi-provider, deliverable and Office acceptance gates.
- Add final-package composer checks for native long-history compaction over 30 seconds, next-round replies, active-page reattachment, cancellation, failure/retry and process restart. Deterministic isolated provider tests do not claim verification of a user relay or authenticated model reasoning.

## 0.41.2-fix5 — 2026-10-29

- Resume the Server multi-provider page check from its persisted business folder
  after process restart, instead of referencing a variable scoped to the earlier
  running process. The fix4 downloaded-package gate caught this harness error
  after 66 checks; its draft was never published and its evidence is retained.
- Enable undefined-variable checking for plain JavaScript release/test scripts.
  The TypeScript preset had disabled this rule globally while tsc did not check
  those scripts. Ordinary CI now tests the actual harness and the failed lexical
  scope pattern, before any Windows package build.
- Retain all application recovery, multi-provider, business context, Pi/runtime,
  delivery, duration, artifact, Office, native Plan/Goal and scroll fixes. Both
  Windows packages still require same-commit installation, upgrade, real-page
  replies and post-publication download verification; no failed tag is moved.

See [release notes](docs/releases/v0.41.2-fix5.md).

## 0.41.2-fix4 — 2026-10-29 (unpublished candidate)

- Retain the complete unpublished recovery, draft-identity and multi-provider
  fixes, with independent business/native Agent bindings and durable queues.
- Preserve owned-process exit codes, signals and explicit cleanup reasons in
  Windows package acceptance; drain stdout and stderr before closing logs and
  collect only scoped OS crash metadata, without process-memory dumps.
- Submit a new unbound Pi draft once through the actual usable Send control;
  record its initial disabled state instead of treating Enter on a disabled
  composer as an accepted input. Historical restoration still tests real input
  during delayed initialization and requires exactly one durable reply.
- Wait for the responsive navigation drawer to mount before dismissing it in
  narrow-layout acceptance, retain the strict hit test and capture failures
  before restoring desktop dimensions.
- Distinguish sudden app interruption from child-first shutdown during handoff
  acceptance. Freeze only the verified CI-owned app writer before terminating
  its tree; native Windows tests retain actual child-failure and pending-request
  recovery as separate states instead of silently retrying a terminal failure.
- Keep the failed fix3 tag and original Server connection-refused evidence.
  Two later diagnostics passed that old-business Server path, but have not explained
  the original process loss. Diagnostic passes do not replace final downloaded
  Server and Setup acceptance for this new commit.

See [release notes](docs/releases/v0.41.2-fix4.md).

## 0.41.2-fix3 — 2026-10-29 (unpublished candidate)

- Keep a draft's live connection when its own native session ID becomes known;
  identity enrichment no longer triggers a destructive target-change restart.
  Genuine session, workspace and Agent changes retain their existing gates.
- Record fixed disconnect call sites and one lifecycle generation across Web
  and desktop APIs, without prompt bodies or credentials.
- Add installed Setup and Server Web composer acceptance with independent GPT
  Responses and Opus Messages Providers: true draft first input, multiple rounds,
  UI model selection, page changes, refresh, restart and fix6 old-business upgrade.
  Assert actual endpoint/model, durable replies, stable session and exact-once input.
- Strip inherited CODEG_* overrides from isolated test children. Preserve both
  Provider configurations and the complete previously unpublished recovery,
  context, runtime, delivery, artifact, Office, duration and scroll fixes.
- Retain failed/cancelled candidate tags. Publication requires fresh dual-package
  inventory, runtime, upgrade, frontend and checksum acceptance for this commit.

See [release notes](docs/releases/v0.41.2-fix3.md) and the
[diagnosis](docs/diagnostics/pi-frontend-lifecycle-20261029.md).

## 0.41.2-fix2 — 2026-10-29 (cancelled candidate)

- Preserve the user's local tab selection when React batches it after a remote
  tab snapshot. Deduplicate saves by their payload instead of dropping the next
  save effect, so later reloads and clients can recover the selected business.
- Strengthen packaged Setup and Server composer acceptance: verify the visible
  business/Agent before sending, establish persisted focus before reload, and
  retain native requests, exact-once replies and delayed-restore queue checks.
  This does not wait for native readiness or warm a connection before input.
- Retain all unpublished recovery, context and runtime-group fixes below.
  The fix1 draft was blocked by full downloaded-package acceptance; its built
  packages and failed evidence are not treated as a completed release.

See [release notes](docs/releases/v0.41.2-fix2.md).

## 0.41.2-fix1 — 2026-10-29 (unpublished candidate)

- Rebase onto upstream v0.41.2 while retaining the custom Windows runtime group,
  business features and all unpublished recovery fixes. Revalidate the merged
  streaming reply layout with the virtualized scroll and delivery gates.

- Preserve business discussion, revision tails, decision reasons, failed attempts
  and file references in bounded cross-Agent context. Record the actual UTF-8
  ranges included in accepted requests; partial turns no longer advance a
  whole-record acknowledgement. Recent context has a reserved budget, and
  accepted ranges are not repeatedly injected while older gaps are completed.
- Add authenticated, snapshot-bounded original business-prose pagination to
  `get_session_info`, including resuming within a long message. Retain the
  legacy compact view, original native sessions and private reasoning/tools.
- Retain the complete applicable-rule bridge needed by existing handoffs, with
  digest deduplication. Do not migrate local Pi infrastructure, change Codex
  profiles/history or add automatic cross-Agent base-configuration syncing.
- Pin Pi ACP 0.0.34-codeg.3 with Pi 1.4.0 and Node 24.16.0 in both packages.
  Its optional native skill-path contract is not automatically configured by
  CodeG; Pi-native rule/skill/memory/ledger setup remains a separate task.
- Retain fix3's restore/readiness and durable composer queue fixes and require
  real fix6 old-business upgrades and multi-round replies in both final Windows
  packages. Keep all prior Pi, external-delivery, duration, artifact, Office,
  native Plan/Goal and virtualized-scroll acceptance gates.

See [release notes](docs/releases/v0.41.2-fix1.md).

## 0.41.1-fix3 — 2026-10-29 (unpublished candidate)

- Read structured Web and desktop restore errors through the shared error
  decoder. Empty business conversations can reach session creation; saved
  sessions retain readable failures and are never silently replaced.
- Retain the verified business/session binding, active-turn cursor and complete
  rule handoff fixes from the unpublished fix1/fix2 candidates.
- Add separate native-runtime upgrades from fix6 data already switched from
  Codex to Pi, in both Setup and Server. Exercise input queued during delayed
  restore, actual replies, subsequent rounds, focus/reload/disconnect, two-way
  handoff and process restart without a programmatic restore warm-up.
- Record the installed page's actual identity/readiness gates and redacted
  persistent-queue states before and after recovery; require the same waiting
  message ID to reach the selected engine once and finish with an empty queue.
- Keep fix4 upgrade/whole-group rollback and all prior business, Pi, delivery,
  duration, Office, native Plan/Goal and virtualized-scroll acceptance. Publish
  only after both downloaded packages pass actual-composer acceptance; retain
  failed draft evidence instead of treating successful builds as acceptance.

See [release notes](docs/releases/v0.41.1-fix3.md).

## 0.41.1-fix2 — 2026-10-28 (unpublished candidate)

- Repair the verified business binding when atomic restore attaches to an active
  first turn, so subsequent composer messages can execute after it completes.
  Retain queue IDs and the running writer in both Windows Setup and Server.
- Return the active lifecycle observation cursor and ignore its older status
  when a newer terminal event has already arrived; preserve snapshot readiness.
- Cover fresh Codex/Pi first-turn binding through Web and desktop events, late
  restore receipts, multi-round sending and recovery. Retain bounded failure
  diagnostics in isolated packaged UI acceptance.
- Include fix1's full rule handoff and packaged-runtime corrections. The fix1
  draft did not pass composer acceptance and was never published.

See [release notes](docs/releases/v0.41.1-fix2.md).

## 0.41.1-fix1 — 2026-10-28 (unpublished candidate)

- Rebase the Windows fork onto upstream v0.41.1, retaining the released business
  Agent, Pi, external delivery, timing, artifacts, Office review and scroll fixes.
- Restore Codex and Pi by stable business conversation identity. Accept verified
  provisional first-prompt readiness and ignore replaced attach callbacks, so
  successive prompts can resume without restarting the app. Show restore errors
  instead of leaving the composer in a waiting state.
- Transfer complete, versioned applicable rule snapshots and repair previously
  truncated handoffs. Advance the delivery receipt only after prompt acceptance;
  resend changed rules and reject unreadable or oversized partial handoffs.
- Validate model catalogs using the packaged native Codex runtime without
  resolving a missing npm codex.js entry; preserve explicit runtime overrides.
- Extend both Windows forms’ isolated acceptance with actual-composer multi-round
  sending, page switches, reload, connection loss and process-restart recovery,
  plus independent checks for long rule tails and updates to existing handoffs.

See [release notes](docs/releases/v0.41.1-fix1.md).

## 0.40.0-fix6 — 2026-10-28

- Retain fix5's packaged Codex, recoverable business handoff, selected Pi version,
  source-instance rules/Skill transfer and conversation-toolbar fixes in both
  Windows forms. The unpublished fix5 tag is preserved for diagnosis.
- Carry terminal initialization failures across connection cleanup, so restoring
  or switching shows the native cause instead of a misleading 300-second timeout.
  Keep real timeouts, interruptions and recoverable warnings distinct.
- Verify unavailable-source rejection without matching localized OS wording;
  retain actual native restore, target readiness, history and UI assertions.
- Select the exact business tab in page acceptance when a device-local draft
  retains focus after reload; retain screenshots and state on source-failure tests.
- Restore the native WebView's real dimensions after narrow-screen acceptance,
  including CDP pages without an emulated viewport.
- Interrupt a deliberately blocked native launcher at its actual startup phase,
  after confirming its child process started; verify the same persisted request
  and native sessions recover after the isolated app restarts.
- Preflight the entire native Windows suite on the source-pinned fix5 packages
  without calling it acceptance of newer binaries. New Server/Setup assets still
  require their own full draft and published acceptance, hashes and inventories.

See [release notes](docs/releases/v0.40.0-fix6.md).

## 0.40.0-fix5 — 2026-10-27

- Ship the same pinned private Codex CLI and self-contained ACP in Windows Setup
  and Server; resolve cached adapters from the selected original package layout
  and use the packaged Node. Preserve explicit runtime paths and user profiles.
- Persist handoff phases and terminal failures, synchronize revision snapshots
  across clients, accept slow valid refreshes, and keep switching available when
  the original engine cannot start. Bound target preparation before activation.
- Transfer the latest task boundary and source-attributed instance/workspace
  instructions, ledger reading requirements and Skill catalog within budgets;
  retain original history and avoid copying credentials or private memory stores.
- Report the selected Pi ACP version/path instead of an unrelated global package;
  prevent global npm actions from replacing the private/custom runtime group.
- Put the current Agent and handoff state at the conversation top; show ten recent
  conversations by default. Retain virtualized history, normal scrolling and all
  previous timing, delivery, Office review, Pi and native execution changes.
- Test each package with its own default runtime: fresh Setup, upgrade, native
  resume, unavailable-source handoff, narrow layout, Server independent data
  upgrade/restart and two-client synchronization. Retain Windows/shared gates.

See [release notes](docs/releases/v0.40.0-fix5.md).

## 0.40.0-fix4 — 2026-10-27

- Retain the stable Codex/Claude/Pi business conversation, external delivery,
  service-tier, whole-task timing, native Plan/Goal, scrolling, deliverable and
  completed Office review changes from the quarantined fix3 package.
- Keep ambiguous same-workspace observations unlinked in the total deliverable
  list, instead of assigning them to an answer or auto-opening an Office review.
  Filter evidenced legacy misattribution without deleting records or files;
  retain explicit declarations, valid review copies and normal inferred previews.
- Prevent the isolated Office converter from updating or installing itself.
  Verify its exact executable path and SHA256 before and after actual use;
  changes affect test children only, preserving users' runtime configuration.
- Require converter-integrity regressions in Windows package acceptance both
  before and after publication, without invalidating the unrelated pinned
  Codex native-build cache. Keep Windows and shared-logic release gates.
- Preserve the failed fix3 release and its evidence as a quarantined prerelease.
  Three optimized allocation-traced full Setup/Server runs did not reproduce
  its one observed 32 GiB allocation failure; three further original-package
  runs with the fixed converter also passed. The cause remains unconfirmed;
  no speculative application change or claim of a memory fix is included.

See [release notes](docs/releases/v0.40.0-fix4.md).

## 0.40.0-fix3 — 2026-10-26 (quarantined prerelease)

- Deliver the Pi/business conversation, external delivery, service tier, artifact,
  Office review and SQLite writer fixes accumulated in the unpublished fix2 candidate.
- Verify rollback history with bounded opaque pages after native Goal execution
  appends to the independent CLI store. Verify the old answer is retained and the
  restored old app renders its actual current tail; missing history still fails.
- Scope frontend error observations to the page check and snapshot their results,
  so later isolated scroll-harness navigation cannot mutate earlier evidence.
- Correlate runtime exceptions with CDP frame identity and initializer source.
  Preserve the exact Tauri path-initializer child-frame exception separately;
  application, unknown and uncorrelated errors still fail package acceptance.
  Pin its actual serialized Windows source SHA256 and retain a licensed fixture.
  Keep isolated test authentication out of sandboxed document frames.
- Retain Windows/shared CI and full Server/Setup installation, upgrade, restart,
  rollback, native protocol and post-publication download verification gates.

See [release notes](docs/releases/v0.40.0-fix3.md).

## 0.40.0-fix2 — 2026-10-26 (unpublished candidate)

- Add Pi to the stable business conversation switcher, preserving independent
  native sessions and per-engine settings, incremental context, shared memory,
  Skills, history and deliverables across handoffs and restart.
- Keep the business title independent of native session auto-titles, including
  delayed title writers, without changing user rename or rollback behavior.
- Acquire the SQLite writer before the tab-version CAS read, so restoring a
  business after restart can wait for background history/index writes. Preserve
  stale-save rejection, metadata clocks and cross-client ordering.
- Verify Pi native session/header/path identities, safely import legacy aliases,
  persist empty sessions, and refuse unverified initialization or restoration.
- Connect Pi 1.4 native MCP, steering and tail clone; exercise actual companion
  delegation, cancellation, result collection and registered deliverable download.
- Add durable conversation-targeted external messages, frozen Agent/session/
  generation/settings, constrained delivery, idempotency, status and cancellation.
  Preserve legacy APIs with strict routing; retain uncertain deliveries without replay.
- Pass Pi Fast / Speed to actual Responses payloads independently of reasoning,
  persist per-model choices, reject unsupported/busy changes, and distinguish
  requested, endpoint-reported and verified upstream tiers without billing claims.
- Ship matching private Node 24.16.0, Pi 1.4.0 and pi-acp 0.0.34-codeg.2 in both
  Windows packages, with source pins, dependency locks, file inventories and licenses.
- Resolve Windows temporary-directory junctions before validating the pinned
  Node ZIP, while retaining traversal and nested-link containment checks.
- Expand isolated Server/Setup protocol, failure, upgrade and asset verification.
  Preserve whole-task timing, native Plan/Goal, all deliverables and scroll fixes.
- Repair inline deliverable refresh field compatibility, stable prompt identities,
  loaded-page/branch scope and first-turn updates; reject ambiguous time-only
  associations while preserving unlinked history and explicit-empty declarations.
- Acquire the SQLite writer before deliverable-set reads, avoiding a reproduced
  native MCP registration failure during concurrent history backfill.
- Drive Office auto-preview from settled final deliveries instead of workspace
  edits. Preserve immutable review bytes during later work, apply final replace/
  clear semantics, deduplicate completion events, and respect user tab choices.
  Keep manual in-app review, clear conversion failures and review-copy downloads.
- Order Office review updates by persistent task sequence, so a late duplicate
  from an older delivery cannot discard the next review while it is loading.
- Keep the active file tab's close target usable in narrow desktop columns;
  verify real pointer hit targets against the reproduced packaged layout.
- Limit self-use CI gates to Windows builds and shared logic. Retain common
  frontend/browser and headless parser/HTTP tests on Linux, move native Plan/Goal
  verification to Windows, and keep both Windows package acceptance gates.
  Linux/macOS clients and Unix installer checks no longer block this fork.

See [release notes](docs/releases/v0.40.0-fix2.md).

## 0.40.0-fix1 — 2026-10-25

- Keep one visible business conversation when switching Codex / Claude Code,
  with separate native sessions, authentication, writers and per-engine settings.
- Queue a running turn's handoff, prepare before publishing, retain the original
  session on failure, and use durable request IDs/revisions and prompt receipts
  for retries, concurrent clients and restart recovery.
- Claim the SQLite writer before checking switch receipts, so concurrent history
  backfill cannot leave a handoff on a stale read snapshot or change retry clocks.
- Refresh the revision once before handoff admission without replacing another
  client's agent intent; render structured rejection details and retain CAS.
- Persist one paged business history, including native task durations, deferred
  attachment sources and deliverables. Preserve rollover identities and existing
  branch creation, restoration and return behavior.
- Pin deferred attachments only in typed CodeG load references; preserve prose,
  quoted references, original tool inputs and unrelated agent metadata.
- Send bounded unseen context and a recent task boundary, with user-confirmed
  scoped memory and one shared project Skill body. Declare omitted context and
  protocol/tool limitations; never import another engine's private native state.
- Preserve native Direct / Plan / Goal controls, explicit Goal pause on handoff,
  ordinary whole-task elapsed labels, the all-deliverables pane and virtualized
  scrolling. Keep live fork identities ahead of stale polled business state.
- Require delivery of the latest bounded business bridge before a native Goal
  can start/resume after a handoff; retain the separate older-history cursor.
- Add additive, reversible business mappings and protocol/failure/concurrency
  regressions, plus real ACP/runtime checks against isolated model fixtures in
  packaged Server Web and installed/upgraded Setup WebView2.
- Rebase on upstream v0.40.0. Publish Windows x64 Server with matching MCP/CLI,
  frontend and migration inventories, and Windows x64 Setup with isolated
  installation, upgrade, resource, dependency and downloaded-asset verification.

See [architecture and limitations](docs/single-business-multi-agent.md) and
[release notes](docs/releases/v0.40.0-fix1.md).

## 0.39.0-fix1 — 2026-10-22

- Restore the conversation-wide deliverables list in the right sidebar under
  Session Details → All deliverables. Keep per-reply outputs available.
- Reuse persisted deliverable history, versions, filters and batch/file actions
  with lazy 25-item pages. Use the persisted conversation ID, reset selection on
  switches and defer reads while the pane is hidden.
- Fit file actions and metadata into narrow panes and preserve disclosure focus.
- Rebase onto upstream v0.39.0 while preserving native whole-task durations,
  Direct / Plan / Goal controls and virtualized scrolling fixes.
- Register two independent same-day migrations in name order for the upstream
  schema-version checks; preserve every released migration name and SQL file.
- Add packaged Server Web and installed Setup regressions for the restored list,
  paging, conversation switching, reloads and minimum-width layout, alongside
  the existing isolated install, upgrade, resource and dependency verification.

See [release notes](docs/releases/v0.39.0-fix1.md).

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
