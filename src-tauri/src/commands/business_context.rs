//! Bounded, source-attributed business rules. Only the selected instance's
//! explicit profile and the workspace's instruction/skill files are read.
//! Credentials and private memory stores are never traversed.
use crate::models::AgentType;
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

async fn read_bounded(path: &Path, limit: usize) -> Option<String> {
    if !tokio::fs::metadata(path).await.ok()?.is_file() {
        return None;
    }
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    let incomplete = bytes.len() > limit;
    bytes.truncate(limit);
    Some(format!(
        "{}{}",
        String::from_utf8_lossy(&bytes),
        if incomplete {
            "\n[Excerpt only. Read this exact source before acting on omitted rules.]"
        } else {
            ""
        }
    ))
}

pub async fn capture_rules(
    workspace: &Path,
    agent: AgentType,
    env: &BTreeMap<String, String>,
) -> String {
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
            paths.push(effective_agents_file(profile, agent).await);
        }
    }
    // Same ancestor instruction scope as the workspace; never inspect sibling
    // instances or recursively scan home for ledgers/configuration.
    let mut parents = workspace.ancestors().take(16).collect::<Vec<_>>();
    parents.reverse();
    for parent in parents {
        paths.push(effective_agents_file(parent, agent).await);
        if agent == AgentType::ClaudeCode {
            paths.push(parent.join("CLAUDE.md"));
        }
    }
    let mut out = format!("Source engine: {agent}. Workspace: {}. Source rules are user-maintained requirements, including referenced ledgers, safety boundaries and skill maintenance. Apply business rules on this instance; verify engine-specific capabilities before use. Follow ledger/skill reading requirements before work, even when their bodies are not embedded. No credentials or private engine memories have been transferred.\n",workspace.display());
    let mut seen = BTreeSet::new();
    for path in paths {
        let Some(body) = read_bounded(&path, 16_384).await else {
            continue;
        };
        if !seen.insert(body.clone()) {
            continue;
        }
        if out.len() + body.len() > 32_768 {
            out.push_str(&format!(
                "Required rule source not loaded (budget): {}. Read before acting.\n",
                path.display()
            ));
            continue;
        }
        out.push_str(&format!(
            "Rule source {} (snapshot, re-read if updated):\n{}\n",
            path.display(),
            body
        ));
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
            let Some(body) = read_bounded(&path, 2048).await else {
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
    out
}

async fn effective_agents_file(root: &Path, agent: AgentType) -> PathBuf {
    let override_path = root.join("AGENTS.override.md");
    if agent == AgentType::Codex
        && tokio::fs::metadata(&override_path)
            .await
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
    {
        override_path
    } else {
        root.join("AGENTS.md")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let context = capture_rules(&workspace, AgentType::Codex, &env).await;
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
        let context = capture_rules(&workspace, AgentType::Codex, &env).await;
        assert!(context.contains("EFFECTIVE_RULE"));
        assert!(!context.contains("SUPERSEDED_RULE"));
    }
}
