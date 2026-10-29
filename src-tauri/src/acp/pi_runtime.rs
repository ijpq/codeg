//! The release-private Pi group never installs into a user's shared npm prefix.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) fn bundled_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    select(exe.parent()?)
}
pub(crate) fn bundled_node() -> Option<PathBuf> {
    bundled_root().map(|root| {
        root.join(if cfg!(windows) {
            "node.exe"
        } else {
            "bin/node"
        })
    })
}
pub(crate) fn adapter_version(path: &Path) -> Option<String> {
    // Probe only the selected entry's own package, never a global npm prefix.
    let parent = path.parent()?;
    let mut packages = vec![];
    if matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some("pi-acp" | "pi-acp.cmd")
    ) {
        packages.push(parent.join("node_modules/pi-acp/package.json"));
    }
    if path.extension().is_some_and(|e| e == "js")
        && parent.file_name().is_some_and(|n| n == "dist")
    {
        packages.push(parent.parent()?.join("package.json"));
    }
    for package in packages {
        let Some(value) = std::fs::read(package)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        else {
            continue;
        };
        if value["name"] == "pi-acp" {
            return value["version"].as_str().map(str::to_owned);
        }
    }
    None
}
fn select(dir: &Path) -> Option<PathBuf> {
    let root = dir.join("pi-runtime");
    let actual: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("source.json")).ok()?).ok()?;
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../../../vendor/pi-runtime/source.json")).ok()?;
    (actual == expected
        && root
            .join(if cfg!(windows) {
                "node.exe"
            } else {
                "bin/node"
            })
            .is_file()
        && root.join("node_modules/pi-acp/dist/index.js").is_file()
        && root
            .join("node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js")
            .is_file())
    .then_some(root)
}
pub(crate) fn adapter(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    if let Some(path) = env
        .get("CODEG_PI_ACP_PATH")
        .cloned()
        .or_else(|| std::env::var("CODEG_PI_ACP_PATH").ok())
        .filter(|path| !path.trim().is_empty())
    {
        return Some(PathBuf::from(path));
    }
    bundled_root().map(|root| {
        root.join(if cfg!(windows) {
            "pi-acp.cmd"
        } else {
            "pi-acp"
        })
    })
}
pub(crate) fn apply(env: &mut BTreeMap<String, String>) {
    let Some(root) = bundled_root() else { return };
    // Preserve an explicit BYO Pi command. Its actual protocol is still checked
    // by the adapter; no shared CLI, extension, model or credential is replaced.
    if !env.contains_key("PI_ACP_PI_COMMAND") && std::env::var_os("PI_ACP_PI_COMMAND").is_none() {
        env.insert(
            "PI_ACP_PI_COMMAND".into(),
            root.join(if cfg!(windows) { "pi.cmd" } else { "pi" })
                .to_string_lossy()
                .into_owned(),
        );
    }
    let selected_pi = env
        .get("PI_ACP_PI_COMMAND")
        .cloned()
        .or_else(|| std::env::var("PI_ACP_PI_COMMAND").ok());
    tracing::info!(runtime_source=%root.display(),pi_command=?selected_pi,bundled_pi_version="1.4.0",adapter_version="0.0.34-codeg.3","[Pi] runtime group and selected Pi command; native identity diagnostics verify custom runtime versions");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_belongs_to_the_selected_entry_not_a_global_install() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("pi-runtime");
        let package = root.join("node_modules/pi-acp");
        std::fs::create_dir_all(package.join("dist")).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"pi-acp","version":"0.0.34-codeg.3"}"#,
        )
        .unwrap();
        assert_eq!(
            adapter_version(&root.join("pi-acp.cmd")).as_deref(),
            Some("0.0.34-codeg.3")
        );
        assert_eq!(
            adapter_version(&package.join("dist/index.js")).as_deref(),
            Some("0.0.34-codeg.3")
        );
        assert_eq!(
            adapter_version(&root.join("unrelated-custom-wrapper.cmd")),
            None
        );
    }
}
