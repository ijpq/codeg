# Pi frontend lifecycle and multi-provider acceptance

## Evidence boundary

The user supplied fix6 Windows log observations: native resume ready without a
business binding/prompt-ready receipt, and later composer prompts stopped by an
explicit frontend disconnect. CLI-initiated GPT and Opus requests succeeded.
The user's running Setup page, its localStorage and its production configuration
have not been accessed or changed. Adding a Provider is a correlation, not an
established cause. A native `ready` or an HTTP response is not business delivery.

## Reproduced defect in the candidate

A new draft connects before its native session ID is known. `session_started`
adds that ID to the existing connection and causes the surface to call connect
again. The original reuse gate compares only the caller's initial request
(without the ID); it treats the same connection's resolved identity as a target
change and calls destructive disconnect, even during a prompt.

Four provider regressions (Pi and Codex, Web attachment and desktop firehose)
failed before the change and passed after it. They observe the disconnect spy
and the original live connection, not merely a displayed connected label.
Three negative cases retain replacement for a genuinely different session,
workspace or Agent. Existing readiness, MCP, owner/viewer and stale-request
gates remain. The resolved identity comparison also requires an exact business
binding; it cannot attach another conversation just because a session ID matches.

The fix6 generic Pi restore path did not activate the stable conversation binding.
The retained atomic business restore/readiness fixes in the earlier unpublished
candidates address that separate boundary. Neither defect alone is claimed to
explain every event in the user's logs.

## Diagnosis and acceptance changes

Destructive frontend disconnects now record fixed call-site names (idle sweep,
target change, abandoned/superseded connect, explicit disconnect, config reapply,
user reconnect and disconnect-all), plus one shared lifecycle generation across
Web and desktop API wrappers. Metadata contains no prompt or credential.

Prior baseline UI checks used published binaries without native model fixtures;
the native business tests also commonly created their conversation by API before
composer input. They did not exercise a truly unbound draft acquiring its first
native/business identity. CLI/ACP success could therefore miss a lifecycle bug
between a real composer and its connection owner.

The combined package harness now uses the installed runtimes and actual visible
composers with two independent local provider APIs: OpenAI Responses and
Anthropic Messages. It selects GPT and the slash-containing Opus model ID through
the settings UI, verifies exactly one native request to the selected endpoint,
one durable user turn and an actual completed reply, preserves both provider
configurations and checks the same native session across model changes. It also
covers tab navigation, reload, process restart and fix6 old-business upgrades in
both Setup and Server Web. Old recovery tests keep their delayed-start queue,
connection-loss, Codex/Pi handoff and exact-once reply assertions. Page readiness
and queue identities are recorded without queue bodies.

Test child processes strip inherited CODEG_* overrides before assigning their
own installation, data, static and runtime paths; user instances are never
located or controlled. Both package manifests, embedded migrations, matching MCP
and pinned native runtimes remain release gates. Full acceptance must be tied to
the release commit and freshly downloaded packages, not an older green workflow.

These deterministic services establish frontend/protocol/tool behavior. They do
not validate the user's authenticated relays, actual service tiers/fees, or a
live model's autonomous Skill/delegation/material-output compliance. Pi-native
infrastructure migration remains outside this task by the user's latest scope.
