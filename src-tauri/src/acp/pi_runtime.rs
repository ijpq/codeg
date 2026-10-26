//! The release-private Pi group never installs into a user's shared npm prefix.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) fn bundled_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    select(exe.parent()?)
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
    tracing::info!(runtime_source=%root.display(),pi_command=?selected_pi,bundled_pi_version="1.4.0",adapter_version="0.0.34-codeg.2","[Pi] runtime group and selected Pi command; native identity diagnostics verify custom runtime versions");
}
