//! Select the release's private CLI without changing installed CLIs or user configuration.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) fn adapter(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    if let Some(path) = env
        .get("CODEG_CODEX_ACP_PATH")
        .cloned()
        .or_else(|| std::env::var("CODEG_CODEX_ACP_PATH").ok())
        .filter(|p| !p.trim().is_empty())
    {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    select(dir, None, false)?;
    let root = dir.join("runtime");
    let actual: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("adapter-source.json")).ok()?).ok()?;
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../../../vendor/codex-adapter/source.json")).ok()?;
    let path = root.join(if cfg!(windows) {
        "codex-acp.cmd"
    } else {
        "codex-acp"
    });
    (actual == expected && path.is_file() && crate::acp::pi_runtime::bundled_node().is_some())
        .then_some(path)
}

pub(crate) fn apply(env: &mut BTreeMap<String, String>) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else { return };
    let inherited = std::env::var("CODEX_PATH").ok();
    let disabled = env.get("CODEG_USE_BUNDLED_CODEX").map(String::as_str) == Some("0")
        || std::env::var("CODEG_USE_BUNDLED_CODEX").as_deref() == Ok("0");
    if let Some(path) = select(
        dir,
        env.get("CODEX_PATH")
            .map(String::as_str)
            .or(inherited.as_deref()),
        disabled,
    ) {
        env.insert("CODEX_PATH".into(), path.to_string_lossy().into_owned());
        env.entry("CODEG_LEGACY_CHECKPOINT_RESUME".into())
            .or_insert_with(|| {
                std::env::var("CODEG_LEGACY_CHECKPOINT_RESUME").unwrap_or_else(|_| "1".into())
            });
        tracing::info!(
            runtime_version = "0.162.1-codeg.1",
            runtime_path = %path.display(),
            "[ACP][restore] private Codex runtime selected"
        );
    } else if !env.get("CODEX_PATH").is_some_and(|p| !p.trim().is_empty())
        && !inherited.is_some_and(|p| !p.trim().is_empty())
    {
        // A deliberately disabled private CLI still uses the user's selected
        // installation, not a missing npm dependency relative to a cached ACP.
        if let Some(path) = crate::commands::acp::resolve_system_agent_binary("codex") {
            env.insert("CODEX_PATH".into(), path.display().to_string());
        }
    }
}

fn select(dir: &Path, explicit: Option<&str>, disabled: bool) -> Option<std::path::PathBuf> {
    if disabled || explicit.is_some_and(|path| !path.trim().is_empty()) {
        return None;
    }
    let path = dir
        .join("runtime")
        .join(if cfg!(windows) { "codex.exe" } else { "codex" });
    let manifest = std::fs::read(dir.join("runtime/source.json")).ok()?;
    let actual: serde_json::Value = serde_json::from_slice(&manifest).ok()?;
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../../../vendor/codex-runtime/source.json")).ok()?;
    (path.is_file() && actual == expected).then_some(path)
}

/// Provenance for the exact private binary, also used by catalog validation.
/// A same-named binary beside an unrelated manifest is never confirmed.
pub(crate) fn binary_version(path: &Path) -> Option<String> {
    let root = path.parent()?.parent()?;
    if select(root, None, false)?.canonicalize().ok()? != path.canonicalize().ok()? {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(include_str!(
        "../../../vendor/codex-runtime/source.json"
    ))
    .ok()?
    .get("version")?
    .as_str()
    .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_runtime_is_optional_and_overrides_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        assert!(select(dir.path(), None, false).is_none());
        let runtime = dir.path().join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        let path = runtime.join(if cfg!(windows) { "codex.exe" } else { "codex" });
        std::fs::write(&path, b"fixture").unwrap();
        assert!(select(dir.path(), None, false).is_none());
        std::fs::write(
            runtime.join("source.json"),
            include_bytes!("../../../vendor/codex-runtime/source.json"),
        )
        .unwrap();
        assert_eq!(select(dir.path(), None, false), Some(path.clone()));
        assert_eq!(binary_version(&path).as_deref(), Some("0.162.1-codeg.1"));
        assert!(select(dir.path(), Some("user-pinned-codex"), false).is_none());
        assert!(select(dir.path(), None, true).is_none());
        std::fs::write(runtime.join("source.json"), b"{}").unwrap();
        assert!(binary_version(&path).is_none());
    }
}
