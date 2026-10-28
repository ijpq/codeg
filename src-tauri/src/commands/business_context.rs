//! Bounded, source-attributed business rules. Only the selected instance's
//! explicit profile and the workspace's instruction/skill files are read.
//! Credentials and private memory stores are never traversed.
use crate::{acp::error::AcpError, models::AgentType};
use sha2::{Digest, Sha256};

const MAX_RULE_FILE_BYTES: usize = 131_072;
pub const MAX_RULE_CONTEXT_BYTES: usize = 196_608;
pub const RULE_CONTEXT_VERSION: u8 = 2;

pub fn rule_digest(contexts: &BTreeMap<String, String>) -> String {
    let mut hasher = Sha256::new();
    hasher.update([RULE_CONTEXT_VERSION]);
    for (source, body) in contexts {
        hasher.update((source.len() as u64).to_le_bytes());
        hasher.update(source.as_bytes());
        hasher.update((body.len() as u64).to_le_bytes());
        hasher.update(body.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use tokio::io::AsyncReadExt;

fn configured(env: &BTreeMap<String, String>, key: &str) -> Option<PathBuf> {
    let value = if cfg!(windows) {
        env.iter()
            .rev()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.clone())
    } else {
        env.get(key).cloned()
    };
    value
        .or_else(|| std::env::var(key).ok())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

async fn read_rule(path: &Path) -> Result<Option<String>, AcpError> {
    // Missing optional sources are normal. An existing unreadable/oversized
    // source is an error: a prefix cannot stand in for complete requirements.
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(AcpError::protocol(format!(
                "Cannot load required rule {}: {e}",
                path.display()
            )))
        }
    };
    let metadata = file
        .metadata()
        .await
        .map_err(|e| AcpError::protocol(format!("Cannot inspect rule {}: {e}", path.display())))?;
    if !metadata.is_file() {
        return Err(AcpError::protocol(format!(
            "Required rule is not a file: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(MAX_RULE_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| {
            AcpError::protocol(format!("Cannot read required rule {}: {e}", path.display()))
        })?;
    if bytes.len() > MAX_RULE_FILE_BYTES {
        return Err(AcpError::protocol(format!("Required rule exceeds the complete-file limit ({MAX_RULE_FILE_BYTES} bytes): {}. No partial rules were delivered.", path.display())));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| AcpError::protocol(format!("Required rule is not UTF-8: {}", path.display())))
}

// Catalog descriptions are discovery metadata, never claimed as loaded rules.
async fn skill_description(path: &Path) -> Option<String> {
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut bytes = Vec::new();
    file.take(2048).read_to_end(&mut bytes).await.ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

pub async fn capture_rules(
    workspace: &Path,
    agent: AgentType,
    env: &BTreeMap<String, String>,
) -> Result<String, AcpError> {
    // Match the launched child's home and explicit env-removal semantics.
    let home = Some(super::acp::pi_child_home(env));
    let profile = match agent {
        AgentType::Codex => {
            configured(env, "CODEX_HOME").or_else(|| home.as_ref().map(|p| p.join(".codex")))
        }
        AgentType::ClaudeCode => configured(env, "CLAUDE_CONFIG_DIR")
            .or_else(|| home.as_ref().map(|p| p.join(".claude"))),
        AgentType::Pi => configured(env, "PI_CODING_AGENT_DIR")
            .or_else(|| home.as_ref().map(|p| p.join(".pi/agent"))),
        _ => None,
    }
    .map(|path| {
        let path = if path.starts_with("~") {
            home.as_ref()
                .map(|home| home.join(path.strip_prefix("~").unwrap()))
                .unwrap_or(path)
        } else {
            path
        };
        if path.is_absolute() {
            path
        } else {
            workspace.join(path)
        }
    });
    let mut paths = Vec::new();
    if let Some(profile) = &profile {
        if agent == AgentType::ClaudeCode {
            paths.push(profile.join("CLAUDE.md"));
        } else {
            paths.push(effective_agents_file(profile, agent).await?);
        }
    }
    // Same ancestor instruction scope as the workspace; never inspect sibling
    // instances or recursively scan home for ledgers/configuration.
    let mut parents = workspace.ancestors().take(16).collect::<Vec<_>>();
    parents.reverse();
    for parent in parents {
        paths.push(effective_agents_file(parent, agent).await?);
        if agent == AgentType::ClaudeCode {
            paths.push(parent.join("CLAUDE.md"));
        }
    }
    let mut out = format!("Complete rule snapshot v{RULE_CONTEXT_VERSION}. Source engine: {agent}. Workspace: {}. Source rules are user-maintained requirements, including referenced ledgers, safety boundaries and skill maintenance. Apply business rules on this instance; verify engine-specific capabilities before use. Follow ledger/skill reading requirements before work, even when their bodies are not embedded. No credentials or private engine memories have been transferred.\n",workspace.display());
    let mut seen = BTreeSet::new();
    for path in paths {
        let Some(body) = read_rule(&path).await? else {
            continue;
        };
        if !seen.insert(body.clone()) {
            continue;
        }
        let record = format!(
            "Rule source {} (complete UTF-8 snapshot):\n{}\n",
            path.display(),
            body
        );
        if out.len() + record.len() > MAX_RULE_CONTEXT_BYTES {
            return Err(AcpError::protocol(format!("Applicable rules exceed the complete-context limit ({MAX_RULE_CONTEXT_BYTES} bytes). No partial rules were delivered.")));
        }
        out.push_str(&record);
    }
    let mut skill_roots = vec![workspace.join(".agents/skills")];
    if let Some(home) = home {
        skill_roots.push(home.join(".agents/skills"));
    }
    if let Some(profile) = profile {
        skill_roots.push(profile.join("skills"));
    }
    out.push_str("Applicable skill catalog: select skills by purpose automatically; read the exact SKILL.md before applying or maintaining them. Skill bodies and source-engine tool/permission assumptions are not automatically loaded or guaranteed on the target. Keep one canonical business template/acceptance standard.\n");
    let mut catalog = String::new();
    for root in skill_roots {
        let Ok(mut entries) = tokio::fs::read_dir(&root).await else {
            continue;
        };
        let mut paths = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            if paths.len() == 64 {
                break;
            }
            paths.push(entry.path().join("SKILL.md"));
        }
        paths.sort();
        for path in paths {
            let Some(body) = skill_description(&path).await else {
                continue;
            };
            let description = body
                .lines()
                .find_map(|l| l.strip_prefix("description:"))
                .unwrap_or("Purpose: read the source skill");
            let row = format!(
                "{} — {}\n",
                path.display(),
                description.trim().chars().take(240).collect::<String>()
            );
            if catalog.len() + row.len() > 8192 {
                break;
            }
            catalog.push_str(&row);
        }
    }
    out.push_str(&catalog);
    Ok(out)
}

async fn effective_agents_file(root: &Path, agent: AgentType) -> Result<PathBuf, AcpError> {
    if agent == AgentType::Codex {
        let override_path = root.join("AGENTS.override.md");
        match tokio::fs::metadata(&override_path).await {
            Ok(metadata) if !metadata.is_file() || metadata.len() > 0 => return Ok(override_path),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(AcpError::protocol(format!(
                    "Cannot inspect required override {}: {e}",
                    override_path.display()
                )))
            }
        }
    }
    Ok(root.join("AGENTS.md"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn incomplete_or_invalid_required_rules_fail_instead_of_becoming_excerpts() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("AGENTS.md");
        tokio::fs::write(&source, vec![b'x'; MAX_RULE_FILE_BYTES + 1])
            .await
            .unwrap();
        let error = read_rule(&source).await.unwrap_err().to_string();
        assert!(error.contains("No partial rules were delivered"));
        tokio::fs::write(&source, [0xff, 0xfe]).await.unwrap();
        assert!(read_rule(&source)
            .await
            .unwrap_err()
            .to_string()
            .contains("UTF-8"));
        assert!(read_rule(&dir.path().join("absent"))
            .await
            .unwrap()
            .is_none());
        assert!(read_rule(dir.path()).await.is_err());
    }

    #[tokio::test]
    async fn full_rule_snapshots_preserve_tail_and_multiple_sources() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("project");
        let profile = dir.path().join("selected");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::create_dir_all(&profile).await.unwrap();
        let rules = format!(
            "{}\n小弟必须通过真实委派工具执行；生成材料只呈现修改结果。",
            "business rules\n".repeat(2000)
        );
        tokio::fs::write(profile.join("AGENTS.md"), &rules)
            .await
            .unwrap();
        tokio::fs::write(
            workspace.join("AGENTS.md"),
            format!(
                "{}\nWorkspace acceptance at the end",
                "project requirements\n".repeat(1600)
            ),
        )
        .await
        .unwrap();
        let env = BTreeMap::from([
            ("CODEX_HOME".into(), profile.display().to_string()),
            ("HOME".into(), dir.path().display().to_string()),
            ("USERPROFILE".into(), dir.path().display().to_string()),
        ]);
        let context = capture_rules(&workspace, AgentType::Codex, &env)
            .await
            .unwrap();
        assert!(context.contains("小弟必须通过真实委派工具执行"));
        assert!(context.contains("生成材料只呈现修改结果"));
        assert!(context.contains("Workspace acceptance at the end"));
        assert!(!context.contains("Excerpt only"));
        assert!(!context.contains("not loaded (budget)"));
    }

    #[tokio::test]
    async fn handoff_rules_load_the_selected_instance_and_preserve_ledger_and_skill_requirements() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("project");
        let selected = dir.path().join("selected-codex");
        let other = dir.path().join("other-codex");
        for root in [&workspace, &selected, &other] {
            tokio::fs::create_dir_all(root).await.unwrap();
        }
        tokio::fs::write(selected.join("AGENTS.md"), "Read ledger-custom.md and its subledger before work. Never restart the production Server.").await.unwrap();
        tokio::fs::write(other.join("AGENTS.md"), "OTHER_INSTANCE_MUST_NOT_LEAK")
            .await
            .unwrap();
        tokio::fs::write(
            workspace.join("AGENTS.md"),
            "Maintain the existing acceptance rules and auto-select applicable skills.",
        )
        .await
        .unwrap();
        let skill = selected.join("skills").join("documents");
        tokio::fs::create_dir_all(&skill).await.unwrap();
        tokio::fs::write(skill.join("SKILL.md"), "---\ndescription: Official document template and acceptance\n---\nPRIVATE_BODY_NOT_AUTOMATICALLY_LOADED").await.unwrap();
        let env = BTreeMap::from([
            ("CODEX_HOME".into(), selected.to_string_lossy().into_owned()),
            ("HOME".into(), dir.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                dir.path().to_string_lossy().into_owned(),
            ),
        ]);
        let context = capture_rules(&workspace, AgentType::Codex, &env)
            .await
            .unwrap();
        assert!(context.contains("Read ledger-custom.md"));
        assert!(context.contains("Never restart the production Server"));
        assert!(context.contains("Official document template and acceptance"));
        assert!(context.contains(&skill.join("SKILL.md").display().to_string()));
        assert!(!context.contains("OTHER_INSTANCE_MUST_NOT_LEAK"));
        assert!(!context.contains("PRIVATE_BODY_NOT_AUTOMATICALLY_LOADED"));
        tokio::fs::write(workspace.join("AGENTS.md"), "LATEST_ACCEPTANCE_REVISION")
            .await
            .unwrap();
        assert!(capture_rules(&workspace, AgentType::Codex, &env)
            .await
            .unwrap()
            .contains("LATEST_ACCEPTANCE_REVISION"));
    }

    #[tokio::test]
    async fn codex_override_and_tilde_profile_follow_the_selected_instance() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let profile = dir.path().join("selected");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::create_dir_all(&profile).await.unwrap();
        tokio::fs::write(profile.join("AGENTS.md"), "SUPERSEDED_RULE")
            .await
            .unwrap();
        tokio::fs::write(profile.join("AGENTS.override.md"), "EFFECTIVE_RULE")
            .await
            .unwrap();
        let env = BTreeMap::from([
            ("CODEX_HOME".into(), "~/selected".into()),
            ("HOME".into(), dir.path().display().to_string()),
            ("USERPROFILE".into(), dir.path().display().to_string()),
        ]);
        let context = capture_rules(&workspace, AgentType::Codex, &env)
            .await
            .unwrap();
        assert!(context.contains("EFFECTIVE_RULE"));
        assert!(!context.contains("SUPERSEDED_RULE"));
    }
}
