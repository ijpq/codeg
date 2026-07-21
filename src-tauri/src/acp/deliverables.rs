//! Explicit, verified final-output declarations from an agent turn.
//!
//! Filesystem watching answers "what changed"; this module answers the separate
//! semantic question "what should be delivered to the user". The MCP tool
//! supplies intent, while codeg canonicalizes every path and verifies that it is
//! a real file or directory inside the conversation workspace before persisting
//! it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::db::service::deliverable_service::{self, VerifiedDeliverable};
use crate::models::ConversationDeliverable;
use crate::web::event_bridge::{emit_event, EventEmitter};

pub const CONVERSATION_DELIVERABLES_CHANGED_EVENT: &str = "conversation://deliverables-changed";
pub const MAX_DELIVERABLES_PER_CALL: usize = 20;
const MAX_TITLE_CHARS: usize = 160;
const MAX_DESCRIPTION_CHARS: usize = 800;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverableInput {
    pub path: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishDeliverablesArgs {
    pub deliverables: Vec<DeliverableInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceptedDeliverable {
    pub id: String,
    pub path: String,
    pub kind: String,
    pub title: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RejectedDeliverable {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishDeliverablesOutcome {
    pub published: bool,
    pub accepted: Vec<AcceptedDeliverable>,
    pub rejected: Vec<RejectedDeliverable>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversationDeliverablesChanged {
    conversation_id: i32,
    deliverable_ids: Vec<String>,
}

pub(crate) fn emit_deliverables_changed(
    emitter: &EventEmitter,
    conversation_id: i32,
    deliverable_ids: Vec<String>,
) {
    emit_event(
        emitter,
        CONVERSATION_DELIVERABLES_CHANGED_EVENT,
        ConversationDeliverablesChanged {
            conversation_id,
            deliverable_ids,
        },
    );
}

#[async_trait]
pub trait SessionDeliverableAccess: Send + Sync {
    async fn publish_deliverables(
        &self,
        parent_connection_id: &str,
        conversation_id: i32,
        workspace_root: &Path,
        items: Vec<DeliverableInput>,
    ) -> PublishDeliverablesOutcome;
}

#[derive(Clone)]
pub struct DbSessionDeliverableAccess {
    db: DatabaseConnection,
    emitter: EventEmitter,
}

impl DbSessionDeliverableAccess {
    pub fn new(db: DatabaseConnection, emitter: EventEmitter) -> Self {
        Self { db, emitter }
    }
}

fn slash_path(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = normalized.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = normalized.strip_prefix("//?/") {
        rest.to_string()
    } else {
        normalized
    }
}

fn rejected(path: impl Into<String>, reason: impl Into<String>) -> RejectedDeliverable {
    RejectedDeliverable {
        path: path.into(),
        reason: reason.into(),
    }
}

fn bounded_text(value: Option<String>, max_chars: usize) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > max_chars {
        return Err(format!("text exceeds {max_chars} characters"));
    }
    Ok(Some(trimmed.to_string()))
}

fn default_title(path: &Path, root: &Path) -> String {
    path.file_name()
        .or_else(|| root.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Deliverable".to_string())
}

fn canonical_candidate(root: &Path, raw: &str) -> Result<PathBuf, String> {
    let requested = PathBuf::from(raw);
    let candidate = if requested.is_absolute() {
        requested
    } else {
        root.join(requested)
    };
    std::fs::canonicalize(&candidate)
        .map_err(|err| format!("path does not exist or cannot be accessed: {err}"))
}

fn relative_to_root<'a>(root: &'a Path, target: &'a Path) -> Result<&'a Path, String> {
    target
        .strip_prefix(root)
        .map_err(|_| "path is outside the conversation workspace".to_string())
}

fn to_accepted(model: &ConversationDeliverable) -> AcceptedDeliverable {
    AcceptedDeliverable {
        id: model.id.clone(),
        path: model.path.clone(),
        kind: model.kind.clone(),
        title: model.title.clone(),
        role: model.role.clone(),
    }
}

#[async_trait]
impl SessionDeliverableAccess for DbSessionDeliverableAccess {
    async fn publish_deliverables(
        &self,
        parent_connection_id: &str,
        conversation_id: i32,
        workspace_root: &Path,
        items: Vec<DeliverableInput>,
    ) -> PublishDeliverablesOutcome {
        let mut outcome = PublishDeliverablesOutcome::default();
        let turn_run_id = match deliverable_service::active_turn_run_id(
            &self.db,
            conversation_id,
            parent_connection_id,
        )
        .await
        {
            Ok(Some(id)) => id,
            Ok(None) => {
                outcome.rejected.push(rejected(
                    "",
                    "the target turn is no longer running; no deliverables were changed",
                ));
                return outcome;
            }
            Err(err) => {
                outcome.rejected.push(rejected(
                    "",
                    format!("failed to resolve the active turn: {err}"),
                ));
                return outcome;
            }
        };
        match deliverable_service::mark_declaration_attempt(&self.db, conversation_id, &turn_run_id)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                outcome.rejected.push(rejected(
                    "",
                    "the target turn ended before the declaration was accepted",
                ));
                return outcome;
            }
            Err(err) => {
                outcome.rejected.push(rejected(
                    "",
                    format!("failed to record the declaration attempt: {err}"),
                ));
                return outcome;
            }
        }
        if items.len() > MAX_DELIVERABLES_PER_CALL {
            outcome.rejected.push(rejected(
                "",
                format!(
                    "a declaration may contain at most {MAX_DELIVERABLES_PER_CALL} deliverables"
                ),
            ));
            return outcome;
        }

        // Clearing is a database-only correction and must remain possible if a
        // temporary workspace has already been removed. Non-empty declarations
        // still require a live canonical root for containment checks.
        let root = if items.is_empty() {
            workspace_root.to_path_buf()
        } else {
            match std::fs::canonicalize(workspace_root) {
                Ok(root) => root,
                Err(err) => {
                    outcome.rejected.push(rejected(
                        "",
                        format!("conversation workspace is unavailable: {err}"),
                    ));
                    return outcome;
                }
            }
        };
        let root_path = slash_path(&root);
        let mut verified = Vec::with_capacity(items.len());
        let mut seen = HashSet::new();

        for item in items {
            let raw_path = item.path.trim().to_string();
            if raw_path.is_empty() {
                outcome
                    .rejected
                    .push(rejected(raw_path, "path must not be empty"));
                continue;
            }
            if raw_path.chars().count() > 4096 {
                outcome
                    .rejected
                    .push(rejected(raw_path, "path is too long"));
                continue;
            }

            let canonical = match canonical_candidate(&root, &raw_path) {
                Ok(path) => path,
                Err(reason) => {
                    outcome.rejected.push(rejected(raw_path, reason));
                    continue;
                }
            };
            if canonical == root {
                outcome.rejected.push(rejected(
                    raw_path,
                    "the workspace root is too broad; declare concrete final outputs",
                ));
                continue;
            }
            let relative = match relative_to_root(&root, &canonical) {
                Ok(path) => path,
                Err(reason) => {
                    outcome.rejected.push(rejected(raw_path, reason));
                    continue;
                }
            };
            let path = if relative.as_os_str().is_empty() {
                ".".to_string()
            } else {
                slash_path(relative)
            };
            if !seen.insert(path.clone()) {
                outcome
                    .rejected
                    .push(rejected(raw_path, "duplicate deliverable path"));
                continue;
            }

            let metadata = match std::fs::metadata(&canonical) {
                Ok(metadata) => metadata,
                Err(err) => {
                    outcome.rejected.push(rejected(
                        raw_path,
                        format!("unable to inspect deliverable: {err}"),
                    ));
                    continue;
                }
            };
            let (kind, size_bytes) = if metadata.is_file() {
                ("file".to_string(), i64::try_from(metadata.len()).ok())
            } else if metadata.is_dir() {
                ("directory".to_string(), None)
            } else {
                outcome.rejected.push(rejected(
                    raw_path,
                    "deliverable must be a regular file or directory",
                ));
                continue;
            };
            let file_name = canonical
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| default_title(&canonical, &root));
            let extension = canonical
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.to_ascii_lowercase());
            let modified_at = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
                .and_then(|duration| {
                    chrono::DateTime::<chrono::Utc>::from_timestamp(
                        i64::try_from(duration.as_secs()).ok()?,
                        duration.subsec_nanos(),
                    )
                });

            let title = match bounded_text(item.title, MAX_TITLE_CHARS) {
                Ok(Some(title)) => title,
                Ok(None) => default_title(&canonical, &root),
                Err(reason) => {
                    outcome.rejected.push(rejected(raw_path, reason));
                    continue;
                }
            };
            let description = match bounded_text(item.description, MAX_DESCRIPTION_CHARS) {
                Ok(description) => description,
                Err(reason) => {
                    outcome.rejected.push(rejected(raw_path, reason));
                    continue;
                }
            };
            let role = item
                .role
                .as_deref()
                .map(str::trim)
                .filter(|role| !role.is_empty())
                .unwrap_or("primary");
            if role != "primary" && role != "supporting" {
                outcome
                    .rejected
                    .push(rejected(raw_path, "role must be `primary` or `supporting`"));
                continue;
            }

            verified.push(VerifiedDeliverable {
                root_path: root_path.clone(),
                path,
                kind,
                title,
                description,
                role: role.to_string(),
                file_name,
                extension,
                size_bytes,
                modified_at,
            });
        }

        // A declaration represents the complete agent-declared final set. Do
        // not partially replace the previous set when even one item is invalid;
        // the tool response tells the agent to resubmit the full corrected set.
        if !outcome.rejected.is_empty() {
            return outcome;
        }
        let verified_paths = verified
            .iter()
            .map(|item| item.path.clone())
            .collect::<Vec<_>>();
        let saved = match deliverable_service::replace_declared_for_turn(
            &self.db,
            conversation_id,
            &turn_run_id,
            verified,
        )
        .await
        {
            Ok(saved) => saved,
            Err(err) => {
                let path = if verified_paths.len() == 1 {
                    verified_paths[0].clone()
                } else {
                    String::new()
                };
                outcome.rejected.push(rejected(
                    path,
                    format!("failed to persist deliverable set: {err}"),
                ));
                return outcome;
            }
        };

        outcome.published = true;
        outcome.accepted = saved.iter().map(to_accepted).collect();
        emit_deliverables_changed(
            &self.emitter,
            conversation_id,
            saved.into_iter().map(|item| item.id).collect(),
        );
        outcome
    }
}

pub fn shared_access(
    db: DatabaseConnection,
    emitter: EventEmitter,
) -> Arc<dyn SessionDeliverableAccess> {
    Arc::new(DbSessionDeliverableAccess::new(db, emitter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::service::artifact_service::{self, NewTurnRun};
    use crate::models::AgentType;
    use sea_orm::EntityTrait;

    #[test]
    fn relative_path_rejects_sibling_workspace() {
        let root = Path::new("/work/project");
        let sibling = Path::new("/work/project-other/report.pdf");
        assert!(relative_to_root(root, sibling).is_err());
    }

    #[test]
    fn bounded_text_trims_and_rejects_oversize_values() {
        assert_eq!(
            bounded_text(Some("  Report  ".into()), 10).unwrap(),
            Some("Report".into())
        );
        assert!(bounded_text(Some("too long".into()), 4).is_err());
    }

    #[test]
    fn slash_path_removes_windows_verbatim_prefixes() {
        assert_eq!(slash_path(Path::new(r"\\?\C:\work\out")), "C:/work/out");
        assert_eq!(
            slash_path(Path::new(r"\\?\UNC\server\share\out")),
            "//server/share/out"
        );
    }

    #[tokio::test]
    async fn publish_is_atomic_and_rejects_escape() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::create_dir_all(workspace.path().join("output")).expect("output directory");
        std::fs::write(workspace.path().join("output/report.pdf"), b"pdf").expect("report file");
        std::fs::write(outside.path().join("secret.txt"), b"secret").expect("outside file");

        let workspace_path = workspace.path().to_string_lossy().to_string();
        let folder_id = crate::db::test_helpers::seed_folder(&db, &workspace_path).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        artifact_service::create_run(
            &db.conn,
            NewTurnRun {
                id: "run-1".into(),
                conversation_id,
                connection_id: "connection-1".into(),
                client_message_id: Some("message-1".into()),
                folder_id: Some(folder_id),
                root_path: std::fs::canonicalize(workspace.path())
                    .expect("canonical workspace")
                    .to_string_lossy()
                    .to_string(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
            },
        )
        .await
        .expect("active run");
        let access = DbSessionDeliverableAccess::new(db.conn.clone(), EventEmitter::Noop);
        let result = access
            .publish_deliverables(
                "connection-1",
                conversation_id,
                workspace.path(),
                vec![
                    DeliverableInput {
                        path: "output/report.pdf".into(),
                        title: Some("Final report".into()),
                        description: None,
                        role: Some("primary".into()),
                    },
                    DeliverableInput {
                        path: outside
                            .path()
                            .join("secret.txt")
                            .to_string_lossy()
                            .to_string(),
                        title: None,
                        description: None,
                        role: None,
                    },
                ],
            )
            .await;

        assert!(!result.published);
        assert!(result.accepted.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert!(result.rejected[0].reason.contains("outside"));
        let run = crate::db::entities::conversation_turn_run::Entity::find_by_id("run-1")
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert!(
            run.deliverables_declared_at.is_some(),
            "a rejected publish call must still suppress fallback inference"
        );

        let persisted = deliverable_service::list_for_conversation(&db.conn, conversation_id)
            .await
            .expect("persisted deliverables");
        assert!(persisted.is_empty());

        let root_result = access
            .publish_deliverables(
                "connection-1",
                conversation_id,
                workspace.path(),
                vec![DeliverableInput {
                    path: ".".into(),
                    title: None,
                    description: None,
                    role: None,
                }],
            )
            .await;
        assert!(root_result.accepted.is_empty());
        assert!(root_result.rejected[0].reason.contains("too broad"));

        let result = access
            .publish_deliverables(
                "connection-1",
                conversation_id,
                workspace.path(),
                vec![DeliverableInput {
                    path: "output/report.pdf".into(),
                    title: Some("Final report".into()),
                    description: None,
                    role: Some("primary".into()),
                }],
            )
            .await;
        assert!(result.published);
        assert_eq!(result.accepted.len(), 1);
        assert_eq!(result.accepted[0].path, "output/report.pdf");

        let persisted = deliverable_service::list_for_conversation(&db.conn, conversation_id)
            .await
            .expect("persisted deliverables");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].title, "Final report");

        let rejected_replacement = access
            .publish_deliverables(
                "connection-1",
                conversation_id,
                workspace.path(),
                vec![DeliverableInput {
                    path: outside
                        .path()
                        .join("secret.txt")
                        .to_string_lossy()
                        .to_string(),
                    title: None,
                    description: None,
                    role: None,
                }],
            )
            .await;
        assert!(!rejected_replacement.published);
        let preserved = deliverable_service::list_for_conversation(&db.conn, conversation_id)
            .await
            .expect("preserved deliverables");
        assert_eq!(preserved.len(), 1);
        assert_eq!(preserved[0].title, "Final report");

        let cleared = access
            .publish_deliverables(
                "connection-1",
                conversation_id,
                workspace.path(),
                Vec::new(),
            )
            .await;
        assert!(cleared.published);
        assert!(cleared.accepted.is_empty());
        assert!(
            deliverable_service::list_for_conversation(&db.conn, conversation_id)
                .await
                .expect("cleared deliverables")
                .is_empty()
        );
    }
}
