//! Select the release's private CLI without changing installed CLIs or user configuration.
use std::{collections::BTreeMap, path::Path};

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
            "[ACP][restore] private Codex runtime selected"
        );
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
        assert_eq!(select(dir.path(), None, false), Some(path));
        assert!(select(dir.path(), Some("user-pinned-codex"), false).is_none());
        assert!(select(dir.path(), None, true).is_none());
    }
}
