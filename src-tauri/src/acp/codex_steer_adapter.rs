//! Runtime compatibility shims for `codex-acp` adapters.
//!
//! `@agentclientprotocol/codex-acp` 1.1.2 already tracks the active Codex
//! app-server turn internally, but does not expose app-server's native
//! `turn/steer` request over ACP. Codeg applies a small, anchor-verified patch
//! to that exact installed bundle and runs the derived copy from Codeg's cache.
//! The installed npm package, Codex configuration, credentials, transcripts,
//! and user settings are never modified.
//!
//! codex-acp 1.1.6+ exposes its own `_session/steering` protocol. The pinned
//! 1.6.2 bundle also has the official Codex app-server `thread/fork` client but
//! does not expose ACP `session/fork`. Codeg applies a second exact-version,
//! anchor-verified patch that maps the standard ACP request to the official
//! persistent fork and advertises the capability. Merge-back remains a Codeg
//! product operation and is deliberately not implemented in the adapter.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::acp::error::AcpError;

const SUPPORTED_ADAPTER_VERSION: &str = "1.1.2";
const NATIVE_FORK_ADAPTER_VERSION: &str = "1.6.2";
const NATIVE_STEERING_MIN_VERSION: (u64, u64, u64) = (1, 1, 6);
const PATCH_REVISION: &str = "codeg-steer-v1";
const NATIVE_FORK_PATCH_REVISION: &str = "codeg-native-thread-fork-citations-v2";

#[derive(Debug, Clone)]
pub struct PreparedCodexSteerAdapter {
    pub node: PathBuf,
    pub script: PathBuf,
    pub node_path: String,
}

struct Replacement {
    before: &'static str,
    after: &'static str,
}

const REPLACEMENTS: &[Replacement] = &[
    Replacement {
        before: r#"  async turnStart(params) {
    return await this.sendRequest({ method: "turn/start", params });
  }
  async runTurn(params, onTurnStarted) {"#,
        after: r#"  async turnStart(params) {
    return await this.sendRequest({ method: "turn/start", params });
  }
  async turnSteer(params) {
    return await this.sendRequest({ method: "turn/steer", params });
  }
  async runTurn(params, onTurnStarted) {"#,
    },
    Replacement {
        before: r#"  resolveTurnInterrupted(params) {
    this.codexClient.resolveTurnInterrupted(params.threadId, params.turnId);
  }"#,
        after: r#"  async sendSteer(request, expectedTurnId, clientUserMessageId) {
    return await this.codexClient.turnSteer({
      threadId: request.sessionId,
      expectedTurnId,
      input: buildPromptItems(request.prompt),
      ...clientUserMessageId ? { clientUserMessageId } : {}
    });
  }
  resolveTurnInterrupted(params) {
    this.codexClient.resolveTurnInterrupted(params.threadId, params.turnId);
  }"#,
    },
    Replacement {
        before: r#"// src/AcpExtensions.ts
var LEGACY_SET_SESSION_MODEL_METHOD = "session/set_model";"#,
        after: r#"// src/AcpExtensions.ts
var LEGACY_SET_SESSION_MODEL_METHOD = "session/set_model";
var STEER_SESSION_METHOD = "session/steer";"#,
    },
    Replacement {
        before: r#"          acp: false,
          http: true,
          sse: false
        }
      },"#,
        after: r#"          acp: false,
          http: true,
          sse: false
        },
        _meta: {
          "codeg/steer": {
            method: "session/steer",
            version: 1
          }
        }
      },"#,
    },
    Replacement {
        before: r#"  async checkAuthorization() {
    const authNeeded = await this.runWithProcessCheck(() => this.codexAcpClient.authRequired());"#,
        after: r#"  async steer(params) {
    const sessionState = this.getSessionState(params.sessionId);
    const expectedTurnId = sessionState.currentTurnId;
    if (!expectedTurnId || !this.activePrompts.has(params.sessionId)) {
      throw RequestError.invalidRequest("CODEG_STEER_NO_ACTIVE_TURN");
    }
    const response = await this.runWithProcessCheck(() => this.codexAcpClient.sendSteer(params, expectedTurnId, params.clientMessageId));
    if (response.turnId !== expectedTurnId) {
      throw RequestError.internalError(`turn/steer returned unexpected turn id ${response.turnId}`);
    }
    return response;
  }
  async checkAuthorization() {
    const authNeeded = await this.runWithProcessCheck(() => this.codexAcpClient.authRequired());"#,
    },
    Replacement {
        before: r#"var legacySetSessionModelParamsParser = external_exports.object({
  sessionId: external_exports.string(),
  modelId: external_exports.string()
}).passthrough();
if (process.argv.includes("--version")) {"#,
        after: r#"var legacySetSessionModelParamsParser = external_exports.object({
  sessionId: external_exports.string(),
  modelId: external_exports.string()
}).passthrough();
var steerSessionParamsParser = external_exports.object({
  sessionId: external_exports.string().min(1),
  prompt: external_exports.array(external_exports.any()).min(1),
  clientMessageId: external_exports.string().min(1).optional()
}).passthrough();
if (process.argv.includes("--version")) {"#,
    },
    Replacement {
        before: r#").onRequest(methods.agent.session.prompt, (ctx) => getAgent().prompt(ctx.params, ctx.signal)).onNotification(methods.agent.session.cancel"#,
        after: r#").onRequest(methods.agent.session.prompt, (ctx) => getAgent().prompt(ctx.params, ctx.signal)).onRequest(STEER_SESSION_METHOD, steerSessionParamsParser, (ctx) => getAgent().steer(ctx.params)).onNotification(methods.agent.session.cancel"#,
    },
];

/// `codex-acp` 1.6.2 already owns the Codex app-server process and tracks all
/// loaded threads, so the safest integration point is the adapter boundary:
/// expose ACP `session/fork`, call the official persistent `thread/fork`, and
/// register an independent adapter-side session state for the returned id.
/// Every anchor is taken from the published npm bundle, not the source tree.
const NATIVE_FORK_REPLACEMENTS: &[Replacement] = &[
    // codex-acp 1.6.2 receives WebSearchItem.results from Codex app-server but
    // drops it while mapping both live and session/load history into ACP. Keep
    // the forward-compatible JSON values in rawInput so CodeG can resolve the
    // private-use citation ids in the assistant text without inventing URLs.
    Replacement {
        before: r#"    query: item.query,
    action: item.action
  };
}
function createCollabAgentToolCallUpdate"#,
        after: r#"    query: item.query,
    action: item.action,
    results: item.results
  };
}
function createCollabAgentToolCallUpdate"#,
    },
    Replacement {
        before: r#"      rawInput: {
        query: item.query,
        action: item.action
      }
    };
  }
  createReviewModeUpdate"#,
        after: r#"      rawInput: {
        query: item.query,
        action: item.action,
        results: item.results
      }
    };
  }
  createReviewModeUpdate"#,
    },
    Replacement {
        before: r#"          delete: {},
          additionalDirectories: {}"#,
        after: r#"          delete: {},
          fork: {
            _meta: {
              "codeg/nativeThreadFork": {
                version: 1,
                method: "thread/fork",
                persistent: true
              }
            }
          },
          additionalDirectories: {}"#,
    },
    Replacement {
        before: r#"  async closeSession(sessionId) {"#,
        after: r#"  async forkSession(sessionId, cwd) {
    return await this.codexClient.threadFork({
      threadId: sessionId,
      cwd
    });
  }
  async closeSession(sessionId) {"#,
    },
    Replacement {
        before: r#"  async loadSession(params) {"#,
        after: r#"  async forkSession(params) {
    if (this.providerUpdate !== null) {
      await this.providerUpdate;
    }
    const source = this.sessions.get(params.sessionId);
    if (!source) {
      throw RequestError.invalidParams(void 0, `Unknown session: ${params.sessionId}`);
    }
    if (source.currentTurnId !== null || this.activePrompts.has(params.sessionId)) {
      throw RequestError.invalidRequest(`Session ${params.sessionId} has an active turn`);
    }
    await this.checkAuthorization();
    logger.log("Forking session with native thread/fork...", {
      sourceSessionId: params.sessionId
    });
    const response = await this.runWithProcessCheck(() =>
      this.codexAcpClient.forkSession(params.sessionId, params.cwd)
    );
    const sessionId = response.thread.id;
    if (!sessionId || sessionId === params.sessionId) {
      throw RequestError.internalError("thread/fork did not return an independent session");
    }
    const sessionState = {
      ...source,
      sessionId,
      currentTurnId: null,
      lastTokenUsage: null,
      totalTokenUsage: null,
      goalRevision: 0,
      sessionTitle: null,
      sessionTitleSource: "unset",
      cwd: params.cwd,
      availableModels: [...source.availableModels],
      supportedReasoningEfforts: [...source.supportedReasoningEfforts],
      supportedInputModalities: [...source.supportedInputModalities],
      additionalDirectories: [...source.additionalDirectories],
      mcpServers: source.mcpServers ? [...source.mcpServers] : void 0,
      sessionMcpServers: source.sessionMcpServers ? [...source.sessionMcpServers] : void 0
    };
    this.sessions.set(sessionId, sessionState);
    this.publishAvailableCommandsAsync(sessionState);
    this.publishCurrentGoalAsync(sessionState, this.getSessionGeneration(sessionId));
    logger.log("Native thread fork created", {
      sourceSessionId: params.sessionId,
      branchSessionId: sessionId
    });
    return {
      sessionId,
      models: this.createModelState(sessionState.availableModels, sessionState.currentModelId),
      modes: sessionState.agentMode.toSessionModeState(),
      ...this.createSessionConfigOptionsResponse(sessionState)
    };
  }
  async loadSession(params) {"#,
    },
    Replacement {
        before: r#").onRequest(methods.agent.session.load, (ctx) => getAgent().loadSession(ctx.params)).onRequest(methods.agent.session.list"#,
        after: r#").onRequest(methods.agent.session.load, (ctx) => getAgent().loadSession(ctx.params)).onRequest(methods.agent.session.fork, (ctx) => getAgent().forkSession(ctx.params)).onRequest(methods.agent.session.list"#,
    },
];

/// Apply every patch anchor exactly once. Refusing partial/ambiguous matches is
/// what makes an unknown adapter version degrade to ordinary ACP safely instead
/// of starting a subtly corrupted process.
pub(crate) fn patch_bundle(source: &str) -> Result<String, String> {
    let mut patched = source.to_string();
    for (index, replacement) in REPLACEMENTS.iter().enumerate() {
        let count = patched.matches(replacement.before).count();
        if count != 1 {
            return Err(format!(
                "codex-acp steer patch anchor {} matched {} times",
                index + 1,
                count
            ));
        }
        patched = patched.replacen(replacement.before, replacement.after, 1);
    }
    Ok(patched)
}

pub(crate) fn patch_native_fork_bundle(source: &str) -> Result<String, String> {
    let mut patched = source.to_string();
    for (index, replacement) in NATIVE_FORK_REPLACEMENTS.iter().enumerate() {
        let count = patched.matches(replacement.before).count();
        if count != 1 {
            return Err(format!(
                "codex-acp native fork patch anchor {} matched {} times",
                index + 1,
                count
            ));
        }
        patched = patched.replacen(replacement.before, replacement.after, 1);
    }
    Ok(patched)
}

fn package_bundle_from_prefix(prefix: &Path) -> PathBuf {
    #[cfg(windows)]
    let node_modules = prefix.join("node_modules");
    #[cfg(not(windows))]
    let node_modules = prefix.join("lib").join("node_modules");

    node_modules
        .join("@agentclientprotocol")
        .join("codex-acp")
        .join("dist")
        .join("index.js")
}

fn npm_prefix_from_launcher(launcher: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        launcher.parent().map(Path::to_path_buf)
    }
    #[cfg(not(windows))]
    {
        launcher.parent()?.parent().map(Path::to_path_buf)
    }
}

fn adapter_version(bundle: &Path) -> Option<String> {
    let package_json = bundle.parent()?.parent()?.join("package.json");
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(package_json).ok()?).ok()?;
    value.get("version")?.as_str().map(str::to_string)
}

fn version_triplet(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split_once('-').map_or(version, |(core, _)| core);
    let mut parts = core.split('.');
    let triplet = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(triplet)
}

fn has_upstream_steering(version: &str) -> bool {
    version_triplet(version).is_some_and(|version| version >= NATIVE_STEERING_MIN_VERSION)
}

fn module_search_path(bundle: &Path, prefix: Option<&Path>) -> String {
    let mut paths = Vec::<PathBuf>::new();
    if let Some(package_root) = bundle.parent().and_then(Path::parent) {
        paths.push(package_root.join("node_modules"));
    }
    if let Some(prefix) = prefix {
        #[cfg(windows)]
        paths.push(prefix.join("node_modules"));
        #[cfg(not(windows))]
        paths.push(prefix.join("lib").join("node_modules"));
    }
    if let Some(existing) = std::env::var_os("NODE_PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths)
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Prepare the derived adapter in Codeg's cache. `Ok(None)` means the installed
/// adapter launches unchanged because it does not match one of the two exact
/// versions this compatibility layer understands.
pub async fn prepare(
    resolved_launcher: &Path,
) -> Result<Option<PreparedCodexSteerAdapter>, AcpError> {
    // The command can come from the shell's npm prefix, Codeg's managed
    // fallback prefix, or a GUI PATH that differs from `npm prefix -g`.
    // Prefer the prefix implied by the launcher so the bundle we patch is the
    // same one Codeg would otherwise execute. The canonical path covers the
    // usual POSIX npm symlink directly into `dist/index.js`.
    let current_prefix = crate::commands::acp::cached_npm_global_prefix().await;
    let mut candidates = Vec::<(PathBuf, Option<PathBuf>)>::new();
    if let Some(prefix) = npm_prefix_from_launcher(resolved_launcher) {
        candidates.push((package_bundle_from_prefix(&prefix), Some(prefix)));
    }
    if let Some(prefix) = crate::process::user_npm_prefix() {
        candidates.push((package_bundle_from_prefix(&prefix), Some(prefix)));
    }
    if let Some(prefix) = current_prefix {
        candidates.push((package_bundle_from_prefix(&prefix), Some(prefix)));
    }
    if let Ok(canonical) = std::fs::canonicalize(resolved_launcher) {
        candidates.push((canonical, None));
    }
    let selected = candidates
        .into_iter()
        .find(|(candidate, _)| candidate.is_file());
    let Some((bundle, package_prefix)) = selected else {
        tracing::warn!(
            "[ACP][Codex] adapter compatibility disabled: agentclientprotocol codex-acp bundle not found"
        );
        return Ok(None);
    };

    let version = adapter_version(&bundle);
    let patch_kind = match version.as_deref() {
        Some(NATIVE_FORK_ADAPTER_VERSION) => "native_fork",
        Some(SUPPORTED_ADAPTER_VERSION) => "legacy_steer",
        _ => {
            if version.as_deref().is_some_and(has_upstream_steering) {
                tracing::warn!(
                    version = version.as_deref().unwrap_or_default(),
                    patchable_fork_version = NATIVE_FORK_ADAPTER_VERSION,
                    "[ACP][Codex] adapter has upstream steering but no verified ACP session/fork bridge; launching unchanged"
                );
            } else {
                tracing::warn!(
                    version = ?version,
                    patchable_legacy_steer_version = SUPPORTED_ADAPTER_VERSION,
                    patchable_fork_version = NATIVE_FORK_ADAPTER_VERSION,
                    "[ACP][Codex] adapter compatibility patch unavailable; launching unchanged"
                );
            }
            return Ok(None);
        }
    };

    let source = std::fs::read_to_string(&bundle).map_err(|error| {
        AcpError::SpawnFailed(format!("failed to read codex-acp adapter: {error}"))
    })?;
    let patched = match if patch_kind == "native_fork" {
        patch_native_fork_bundle(&source)
    } else {
        patch_bundle(&source)
    } {
        Ok(patched) => patched,
        Err(error) => {
            tracing::warn!(
                patch_kind,
                "[ACP][Codex] compatibility patch disabled: {error}"
            );
            return Ok(None);
        }
    };

    let (patch_revision, cache_version) = if patch_kind == "native_fork" {
        (NATIVE_FORK_PATCH_REVISION, NATIVE_FORK_ADAPTER_VERSION)
    } else {
        (PATCH_REVISION, SUPPORTED_ADAPTER_VERSION)
    };

    let mut hash = Sha256::new();
    hash.update(source.as_bytes());
    hash.update(patch_revision.as_bytes());
    let digest = format!("{:x}", hash.finalize());
    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| AcpError::SpawnFailed("system cache directory unavailable".into()))?
        .join("app.codeg")
        .join("acp-adapters")
        .join(format!("codex-acp-{cache_version}"));
    std::fs::create_dir_all(&cache_dir).map_err(|error| {
        AcpError::SpawnFailed(format!("failed to create adapter cache: {error}"))
    })?;
    let script = cache_dir.join(format!("index-{}.mjs", &digest[..16]));
    if !script.is_file() {
        let temporary = cache_dir.join(format!(
            ".index-{}-{}.tmp",
            &digest[..16],
            std::process::id()
        ));
        std::fs::write(&temporary, patched).map_err(|error| {
            AcpError::SpawnFailed(format!("failed to write adapter cache: {error}"))
        })?;
        match std::fs::rename(&temporary, &script) {
            Ok(()) => {}
            Err(_) if script.is_file() => {
                let _ = std::fs::remove_file(&temporary);
            }
            Err(error) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(AcpError::SpawnFailed(format!(
                    "failed to publish adapter cache: {error}"
                )));
            }
        }
    }

    let Some(node) = crate::commands::acp::resolve_npx_command("node").await else {
        return Err(AcpError::SdkNotInstalled(
            "Node.js is not installed. Please install it in Agent Settings.".into(),
        ));
    };

    tracing::info!(
        version = version.as_deref().unwrap_or_default(),
        patch_kind,
        derived_script = %script.display(),
        "[ACP][Codex] launching verified derived adapter"
    );

    Ok(Some(PreparedCodexSteerAdapter {
        node,
        script,
        node_path: module_search_path(&bundle, package_prefix.as_deref()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_bundle() -> String {
        REPLACEMENTS
            .iter()
            .map(|replacement| replacement.before)
            .collect::<Vec<_>>()
            .join("\n__separator__\n")
    }

    fn native_fork_fixture_bundle() -> String {
        NATIVE_FORK_REPLACEMENTS
            .iter()
            .map(|replacement| replacement.before)
            .collect::<Vec<_>>()
            .join("\n__separator__\n")
    }

    #[test]
    fn patch_adds_native_turn_steer_chain() {
        let patched = patch_bundle(&fixture_bundle()).expect("fixture patches");
        assert!(patched.contains("method: \"turn/steer\""));
        assert!(patched.contains("expectedTurnId"));
        assert!(patched.contains("clientUserMessageId"));
        assert!(patched.contains("response.turnId !== expectedTurnId"));
        assert!(patched.contains("onRequest(STEER_SESSION_METHOD"));
        assert!(patched.contains("CODEG_STEER_NO_ACTIVE_TURN"));
        assert!(patched.contains("\"codeg/steer\""));
    }

    #[test]
    fn patch_refuses_unknown_or_already_patched_bundle() {
        assert!(patch_bundle("unknown").is_err());
        let once = patch_bundle(&fixture_bundle()).expect("first patch");
        assert!(patch_bundle(&once).is_err());
    }

    #[test]
    fn patch_exposes_official_persistent_thread_fork_over_acp() {
        let patched = patch_native_fork_bundle(&native_fork_fixture_bundle())
            .expect("published 1.6.2 anchors patch");
        assert!(patched.contains("fork: {"));
        assert!(patched.contains("\"codeg/nativeThreadFork\""));
        assert!(patched.contains("this.codexClient.threadFork"));
        assert!(patched.contains("threadId: sessionId"));
        assert!(patched.contains("this.sessions.set(sessionId, sessionState)"));
        assert!(patched.contains("methods.agent.session.fork"));
        assert!(patched.contains("thread/fork did not return an independent session"));
    }

    #[test]
    fn native_fork_patch_refuses_unknown_or_already_patched_bundle() {
        assert!(patch_native_fork_bundle("unknown").is_err());
        let once = patch_native_fork_bundle(&native_fork_fixture_bundle())
            .expect("first native fork patch");
        assert!(patch_native_fork_bundle(&once).is_err());
    }

    #[test]
    fn published_native_fork_bundle_matches_when_fixture_path_is_supplied() {
        let Ok(path) = std::env::var("CODEG_CODEX_ACP_PUBLISHED_BUNDLE") else {
            return;
        };
        let source = std::fs::read_to_string(path).expect("read published codex-acp bundle");
        let patched = patch_native_fork_bundle(&source)
            .expect("published codex-acp 1.6.2 bundle must match every verified anchor");
        assert!(patched.contains("methods.agent.session.fork"));
        assert!(patched.contains("this.codexClient.threadFork"));
        assert!(patched.matches("results: item.results").count() >= 2);

        let fixture = tempfile::Builder::new()
            .suffix(".mjs")
            .tempfile()
            .expect("create patched bundle fixture");
        std::fs::write(fixture.path(), patched).expect("write patched bundle fixture");
        let status = std::process::Command::new("node")
            .arg("--check")
            .arg(fixture.path())
            .status()
            .expect("run node syntax check");
        assert!(
            status.success(),
            "patched published bundle must parse as ESM"
        );
    }

    #[test]
    fn recognizes_versions_with_upstream_steering() {
        assert!(!has_upstream_steering("1.1.5"));
        assert!(has_upstream_steering("1.1.6"));
        assert!(has_upstream_steering("1.1.7"));
        assert!(has_upstream_steering("1.2.0-beta.1"));
        assert!(has_upstream_steering("2.0.0"));
        assert!(!has_upstream_steering("not-a-version"));
    }

    #[cfg(not(windows))]
    #[test]
    fn infers_posix_npm_prefix_from_bin_launcher() {
        assert_eq!(
            npm_prefix_from_launcher(Path::new("/opt/node/bin/codex-acp")),
            Some(PathBuf::from("/opt/node"))
        );
    }
}
