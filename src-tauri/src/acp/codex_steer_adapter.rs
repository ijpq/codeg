//! Runtime compatibility shim for Codeg's pinned `codex-acp` adapter.
//!
//! `@agentclientprotocol/codex-acp` 1.1.2 already tracks the active Codex
//! app-server turn internally, but does not expose app-server's native
//! `turn/steer` request over ACP. Codeg applies a small, anchor-verified patch
//! to that exact installed bundle and runs the derived copy from Codeg's cache.
//! The installed npm package, Codex configuration, credentials, transcripts,
//! and user settings are never modified.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::acp::error::AcpError;

const SUPPORTED_ADAPTER_VERSION: &str = "1.1.2";
const PATCH_REVISION: &str = "codeg-steer-v1";

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
/// adapter/version cannot be patched; callers launch it unchanged and publish
/// `supports_steer=false`, preserving the existing live-feedback path.
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
            "[ACP][Codex] native steer disabled: agentclientprotocol codex-acp bundle not found"
        );
        return Ok(None);
    };

    let version = adapter_version(&bundle);
    if version.as_deref() != Some(SUPPORTED_ADAPTER_VERSION) {
        tracing::warn!(
            "[ACP][Codex] native steer disabled: installed codex-acp version {:?}, supported {}",
            version,
            SUPPORTED_ADAPTER_VERSION
        );
        return Ok(None);
    }

    let source = std::fs::read_to_string(&bundle).map_err(|error| {
        AcpError::SpawnFailed(format!("failed to read codex-acp adapter: {error}"))
    })?;
    let patched = match patch_bundle(&source) {
        Ok(patched) => patched,
        Err(error) => {
            tracing::warn!("[ACP][Codex] native steer disabled: {error}");
            return Ok(None);
        }
    };

    let mut hash = Sha256::new();
    hash.update(source.as_bytes());
    hash.update(PATCH_REVISION.as_bytes());
    let digest = format!("{:x}", hash.finalize());
    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| AcpError::SpawnFailed("system cache directory unavailable".into()))?
        .join("app.codeg")
        .join("acp-adapters")
        .join(format!("codex-acp-{SUPPORTED_ADAPTER_VERSION}"));
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

    #[cfg(not(windows))]
    #[test]
    fn infers_posix_npm_prefix_from_bin_launcher() {
        assert_eq!(
            npm_prefix_from_launcher(Path::new("/opt/node/bin/codex-acp")),
            Some(PathBuf::from("/opt/node"))
        );
    }
}
