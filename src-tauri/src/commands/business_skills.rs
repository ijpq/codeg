//! One project skill body for both engines. Discovery does not claim loading:
//! a named `$skill` or `/skill` loads its bounded body into the next prompt;
//! other skills are only a catalog the engine can read with its own file tool.
use crate::acp::types::PromptInputBlock;
use std::path::Path;
use tokio::io::AsyncReadExt;

pub async fn shared_skill_context(workspace: &Path, blocks: &[PromptInputBlock]) -> String {
    let root = workspace.join(".agents/skills");
    let Ok(mut entries) = tokio::fs::read_dir(&root).await else {
        return String::new();
    };
    let prompt = blocks
        .iter()
        .filter_map(|b| match b {
            PromptInputBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut paths = Vec::new();
    // Fixed discovery/read budgets, independent of transcript size.
    while let Ok(Some(entry)) = entries.next_entry().await {
        if paths.len() >= 64 {
            break;
        }
        if entry
            .file_type()
            .await
            .is_ok_and(|t| t.is_dir() || t.is_symlink())
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    let mut output = String::new();
    for path in paths {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let source = path.join("SKILL.md");
        let Ok(file) = tokio::fs::File::open(&source).await else {
            continue;
        };
        let mut bytes = Vec::new();
        if file.take(16_385).read_to_end(&mut bytes).await.is_err() {
            continue;
        }
        let body = String::from_utf8_lossy(&bytes);
        let named = prompt.split_whitespace().any(|token| {
            let token = token.trim_matches(|c: char| ",.;:!?，。；：！？()（）\"'`".contains(c));
            token == format!("${name}") || token == format!("/{name}")
        });
        let record = if named && bytes.len() <= 16_384 {
            format!("Loaded shared skill {name} from {} (project file, not private engine memory):\n{body}\n", source.display())
        } else {
            format!("Shared skill {name}: {}. Body not loaded{}; read SKILL.md with your file tool before applying it.\n", source.display(), if named { " (exceeds bridge limit)" } else { "" })
        };
        if output.len() + record.len() > 24_576 {
            output.push_str("[Additional shared skills omitted by the bridge budget; inspect .agents/skills on demand.]\n");
            break;
        }
        output.push_str(&record);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn business_shared_skill_loads_one_body_for_both_engines_and_rereads_updates() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".agents/skills/official-document");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let source = root.join("SKILL.md");
        tokio::fs::write(
            &source,
            "Preserve template A; require the existing acceptance checklist.",
        )
        .await
        .unwrap();
        let blocks = [PromptInputBlock::Text {
            text: "Apply $official-document".into(),
        }];
        for _engine in ["codex", "claude_code"] {
            let context = shared_skill_context(dir.path(), &blocks).await;
            assert!(context.contains("Loaded shared skill official-document"));
            assert!(context.contains("Preserve template A"));
        }
        tokio::fs::write(&source, "Preserve revised template B")
            .await
            .unwrap();
        assert!(shared_skill_context(dir.path(), &blocks)
            .await
            .contains("template B"));
        let catalog = shared_skill_context(dir.path(), &[]).await;
        assert!(catalog.contains("Body not loaded"));
        assert!(!catalog.contains("template B"));
        // Never mix private engine memory or private skill directories.
        let private = dir.path().join(".claude/skills/private");
        tokio::fs::create_dir_all(&private).await.unwrap();
        tokio::fs::write(private.join("SKILL.md"), "PRIVATE_NOT_SHARED")
            .await
            .unwrap();
        assert!(!shared_skill_context(dir.path(), &[])
            .await
            .contains("PRIVATE_NOT_SHARED"));
    }
}
