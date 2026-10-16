//! Explicit, verified final-output declarations from an agent turn.
//!
//! Filesystem watching answers "what changed"; this module answers the separate
//! semantic question "what should be delivered to the user". The MCP tool
//! supplies intent, while codeg canonicalizes every path and verifies that it is
//! a real file or directory inside the conversation workspace before persisting
//! it.

use std::collections::{HashMap, HashSet};
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
pub const MAX_DELIVERABLES_PER_CALL: usize = 200;
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
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub change_kind: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PublishDeliverablesArgs {
    #[serde(default)]
    pub deliverables: Vec<DeliverableInput>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceptedDeliverable {
    pub id: String,
    pub path: String,
    pub kind: String,
    pub title: String,
    pub role: String,
    pub category: String,
    pub change_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RejectedDeliverable {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishDeliverablesOutcome {
    pub request_id: String,
    pub declaration_status: String,
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
        request_id: &str,
        parent_connection_id: &str,
        conversation_id: i32,
        workspace_root: &Path,
        args: PublishDeliverablesArgs,
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
        category: model.category.clone(),
        change_kind: model.change_kind.clone(),
    }
}

fn recorded_relative_path(root: &Path, raw: &str) -> Result<String, String> {
    let requested = PathBuf::from(raw);
    let relative = if requested.is_absolute() {
        requested
            .strip_prefix(root)
            .map_err(|_| "path is outside the conversation workspace".to_string())?
            .to_path_buf()
    } else {
        requested
    };
    if relative.as_os_str().is_empty()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err("path contains an unsafe component".to_string());
    }
    Ok(slash_path(&relative))
}

fn default_category(extension: Option<&str>) -> &'static str {
    if matches!(
        extension,
        Some(
            "docx"
                | "xlsx"
                | "pptx"
                | "pdf"
                | "png"
                | "jpg"
                | "jpeg"
                | "webp"
                | "zip"
                | "rar"
                | "7z"
                | "csv"
                | "tsv"
                | "xml"
                | "svg"
                | "mp4"
                | "mov"
                | "avi"
                | "mkv"
                | "webm"
                | "m4v"
                | "mp3"
                | "wav"
                | "m4a"
                | "flac"
        )
    ) {
        "standalone_output"
    } else {
        "code_change"
    }
}

async fn record_outcome(
    db: &DatabaseConnection,
    request_id: &str,
    conversation_id: i32,
    turn_run_id: &str,
    payload_json: &str,
    outcome: &PublishDeliverablesOutcome,
) {
    let error = (!outcome.rejected.is_empty()).then(|| {
        outcome
            .rejected
            .iter()
            .map(|item| {
                if item.path.is_empty() {
                    item.reason.clone()
                } else {
                    format!("{}: {}", item.path, item.reason)
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
    });
    if let Err(err) = deliverable_service::mark_declaration_result(
        db,
        conversation_id,
        turn_run_id,
        &outcome.declaration_status,
        error,
    )
    .await
    {
        tracing::error!(
            "failed to record deliverable declaration state for run {}: {}",
            turn_run_id,
            err
        );
    }
    let outcome_json = serde_json::to_string(outcome).unwrap_or_else(|_| "{}".to_string());
    if let Err(err) = deliverable_service::cache_declaration_outcome(
        db,
        request_id,
        conversation_id,
        turn_run_id,
        &outcome.declaration_status,
        payload_json,
        &outcome_json,
    )
    .await
    {
        tracing::error!(
            "failed to cache deliverable declaration outcome {}: {}",
            request_id,
            err
        );
    }
}

#[async_trait]
impl SessionDeliverableAccess for DbSessionDeliverableAccess {
    async fn publish_deliverables(
        &self,
        request_id: &str,
        parent_connection_id: &str,
        conversation_id: i32,
        workspace_root: &Path,
        args: PublishDeliverablesArgs,
    ) -> PublishDeliverablesOutcome {
        if let Ok(Some(cached)) =
            deliverable_service::cached_declaration_outcome(&self.db, request_id, conversation_id)
                .await
        {
            if let Ok(outcome) = serde_json::from_str(&cached) {
                return outcome;
            }
        }

        let payload_json = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
        let mut outcome = PublishDeliverablesOutcome {
            request_id: request_id.to_string(),
            declaration_status: "failed".to_string(),
            ..Default::default()
        };
        let run = match deliverable_service::active_turn_run(
            &self.db,
            conversation_id,
            parent_connection_id,
        )
        .await
        {
            Ok(Some(run)) => run,
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
        let turn_run_id = run.id.clone();
        let mode = args.mode.as_deref().unwrap_or("merge").trim();
        if !matches!(mode, "merge" | "replace" | "clear") {
            outcome
                .rejected
                .push(rejected("", "mode must be `merge`, `replace`, or `clear`"));
            record_outcome(
                &self.db,
                request_id,
                conversation_id,
                &turn_run_id,
                &payload_json,
                &outcome,
            )
            .await;
            return outcome;
        }
        if args.deliverables.len() > MAX_DELIVERABLES_PER_CALL {
            outcome.rejected.push(rejected(
                "",
                format!(
                    "a declaration may contain at most {MAX_DELIVERABLES_PER_CALL} deliverables"
                ),
            ));
            record_outcome(
                &self.db,
                request_id,
                conversation_id,
                &turn_run_id,
                &payload_json,
                &outcome,
            )
            .await;
            return outcome;
        }
        if args.remove.len() > MAX_DELIVERABLES_PER_CALL {
            outcome.rejected.push(rejected(
                "",
                format!(
                    "a declaration may remove at most {MAX_DELIVERABLES_PER_CALL} deliverables"
                ),
            ));
            record_outcome(
                &self.db,
                request_id,
                conversation_id,
                &turn_run_id,
                &payload_json,
                &outcome,
            )
            .await;
            return outcome;
        }
        if mode == "clear" && (!args.deliverables.is_empty() || !args.remove.is_empty()) {
            outcome.rejected.push(rejected(
                "",
                "clear mode requires empty `deliverables` and `remove` arrays",
            ));
            record_outcome(
                &self.db,
                request_id,
                conversation_id,
                &turn_run_id,
                &payload_json,
                &outcome,
            )
            .await;
            return outcome;
        }

        let root = if mode == "clear" {
            PathBuf::from(&run.root_path)
        } else {
            match std::fs::canonicalize(workspace_root) {
                Ok(root) => root,
                Err(err) => {
                    outcome.rejected.push(rejected(
                        "",
                        format!("conversation workspace is unavailable: {err}"),
                    ));
                    record_outcome(
                        &self.db,
                        request_id,
                        conversation_id,
                        &turn_run_id,
                        &payload_json,
                        &outcome,
                    )
                    .await;
                    return outcome;
                }
            }
        };
        let root_path = slash_path(&root);
        let changes =
            crate::db::service::artifact_service::list_changes_for_run(&self.db, &turn_run_id)
                .await
                .unwrap_or_default();
        let change_map = changes
            .into_iter()
            .map(|change| (change.path.clone(), change))
            .collect::<HashMap<_, _>>();
        let mut verified = Vec::with_capacity(args.deliverables.len());
        let mut seen = HashSet::new();
        let requested_count = args.deliverables.len();

        for item in args.deliverables {
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

            let requested_change_kind = item
                .change_kind
                .as_deref()
                .map(str::trim)
                .filter(|kind| !kind.is_empty());
            if requested_change_kind
                .is_some_and(|kind| !matches!(kind, "created" | "modified" | "deleted" | "renamed"))
            {
                outcome.rejected.push(rejected(
                    raw_path,
                    "change_kind must be `created`, `modified`, `deleted`, or `renamed`",
                ));
                continue;
            }
            let deleting = requested_change_kind == Some("deleted");
            let (canonical, path, metadata) = if deleting {
                let path = match recorded_relative_path(&root, &raw_path) {
                    Ok(path) => path,
                    Err(reason) => {
                        outcome.rejected.push(rejected(raw_path, reason));
                        continue;
                    }
                };
                let confirmed_deleted = change_map.get(&path).is_some_and(|change| {
                    change.kind
                        == crate::db::entities::conversation_turn_file_change::ConversationTurnFileChangeKind::Deleted
                        && change.final_exists == Some(false)
                });
                if !confirmed_deleted {
                    outcome.rejected.push(rejected(
                        raw_path,
                        "deleted paths must be confirmed by this turn's filesystem evidence",
                    ));
                    continue;
                }
                (root.join(&path), path, None)
            } else {
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
                        "the workspace root is too broad; declare concrete changed files or outputs",
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
                let path = slash_path(relative);
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
                (canonical, path, Some(metadata))
            };
            if !seen.insert(path.clone()) {
                outcome
                    .rejected
                    .push(rejected(raw_path, "duplicate deliverable path"));
                continue;
            }

            let (kind, size_bytes) = if deleting {
                ("file".to_string(), None)
            } else if metadata.as_ref().is_some_and(|metadata| metadata.is_file()) {
                (
                    "file".to_string(),
                    metadata
                        .as_ref()
                        .and_then(|metadata| i64::try_from(metadata.len()).ok()),
                )
            } else if metadata.as_ref().is_some_and(|metadata| metadata.is_dir()) {
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
                .as_ref()
                .and_then(|metadata| metadata.modified().ok())
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
            let category = item
                .category
                .as_deref()
                .map(str::trim)
                .filter(|category| !category.is_empty())
                .unwrap_or_else(|| {
                    if kind == "directory" {
                        "standalone_output"
                    } else {
                        default_category(extension.as_deref())
                    }
                });
            if !matches!(category, "code_change" | "standalone_output") {
                outcome.rejected.push(rejected(
                    raw_path,
                    "category must be `code_change` or `standalone_output`",
                ));
                continue;
            }
            let role = item
                .role
                .as_deref()
                .map(str::trim)
                .filter(|role| !role.is_empty())
                .unwrap_or(if category == "code_change" {
                    "supporting"
                } else {
                    "primary"
                });
            if role != "primary" && role != "supporting" {
                outcome
                    .rejected
                    .push(rejected(raw_path, "role must be `primary` or `supporting`"));
                continue;
            }
            if deleting && category != "code_change" {
                outcome.rejected.push(rejected(
                    raw_path,
                    "deleted entries must use category `code_change`",
                ));
                continue;
            }
            let change_kind = requested_change_kind
                .map(str::to_string)
                .or_else(|| {
                    change_map.get(&path).map(|change| match change.kind {
                        crate::db::entities::conversation_turn_file_change::ConversationTurnFileChangeKind::Created => "created",
                        crate::db::entities::conversation_turn_file_change::ConversationTurnFileChangeKind::Modified => "modified",
                        crate::db::entities::conversation_turn_file_change::ConversationTurnFileChangeKind::Deleted => "deleted",
                        crate::db::entities::conversation_turn_file_change::ConversationTurnFileChangeKind::Renamed => "renamed",
                    }.to_string())
                })
                .unwrap_or_else(|| {
                    if category == "code_change" {
                        "modified".to_string()
                    } else {
                        "created".to_string()
                    }
                });

            verified.push(VerifiedDeliverable {
                root_path: root_path.clone(),
                path,
                kind,
                title,
                description,
                role: role.to_string(),
                category: category.to_string(),
                change_kind,
                file_name,
                extension,
                size_bytes,
                modified_at,
                is_valid: !deleting,
                invalid_reason: deleting.then(|| "deleted".to_string()),
            });
        }

        let mut remove_paths = Vec::new();
        let mut seen_remove_paths = HashSet::new();
        for raw_path in &args.remove {
            match recorded_relative_path(&root, raw_path.trim()) {
                Ok(path) if seen.contains(&path) => outcome.rejected.push(rejected(
                    raw_path,
                    "the same path cannot be published and removed in one declaration",
                )),
                Ok(path) if !seen_remove_paths.insert(path.clone()) => outcome
                    .rejected
                    .push(rejected(raw_path, "duplicate removal path")),
                Ok(path) => remove_paths.push(path),
                Err(reason) => outcome.rejected.push(rejected(raw_path, reason)),
            }
        }

        if (mode == "replace" && !outcome.rejected.is_empty())
            || (verified.is_empty() && remove_paths.is_empty() && !outcome.rejected.is_empty())
            || (requested_count > 0 && verified.is_empty() && args.remove.is_empty())
        {
            record_outcome(
                &self.db,
                request_id,
                conversation_id,
                &turn_run_id,
                &payload_json,
                &outcome,
            )
            .await;
            return outcome;
        }
        let verified_paths = verified
            .iter()
            .map(|item| item.path.clone())
            .collect::<Vec<_>>();
        let saved = match deliverable_service::save_declared_for_turn(
            &self.db,
            conversation_id,
            &turn_run_id,
            verified,
            matches!(mode, "replace" | "clear"),
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
                record_outcome(
                    &self.db,
                    request_id,
                    conversation_id,
                    &turn_run_id,
                    &payload_json,
                    &outcome,
                )
                .await;
                return outcome;
            }
        };
        if let Err(err) = deliverable_service::remove_declared_paths_for_turn(
            &self.db,
            conversation_id,
            &turn_run_id,
            &root_path,
            &remove_paths,
        )
        .await
        {
            outcome.rejected.push(rejected(
                "",
                format!("failed to remove deliverables: {err}"),
            ));
        }

        outcome.published = true;
        outcome.accepted = saved.iter().map(to_accepted).collect();
        outcome.declaration_status = if outcome.rejected.is_empty() {
            if saved.is_empty() && remove_paths.is_empty() {
                "success_empty"
            } else {
                "success"
            }
        } else {
            "partial"
        }
        .to_string();
        record_outcome(
            &self.db,
            request_id,
            conversation_id,
            &turn_run_id,
            &payload_json,
            &outcome,
        )
        .await;
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

    fn input(path: impl Into<String>) -> DeliverableInput {
        DeliverableInput {
            path: path.into(),
            title: None,
            description: None,
            role: None,
            category: None,
            change_kind: None,
        }
    }

    fn args(deliverables: Vec<DeliverableInput>, mode: Option<&str>) -> PublishDeliverablesArgs {
        PublishDeliverablesArgs {
            deliverables,
            mode: mode.map(str::to_string),
            remove: Vec::new(),
        }
    }

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
    async fn publish_merges_valid_items_and_rejects_escape_without_losing_state() {
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
                prompt_fingerprint: None,
                folder_id: Some(folder_id),
                root_path: std::fs::canonicalize(workspace.path())
                    .expect("canonical workspace")
                    .to_string_lossy()
                    .to_string(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
                expectation_json:
                    r#"{"publish_required":true,"expects_code_changes":true,"requested_paths":[]}"#
                        .into(),
            },
        )
        .await
        .expect("active run");
        let access = DbSessionDeliverableAccess::new(db.conn.clone(), EventEmitter::Noop);
        let result = access
            .publish_deliverables(
                "request-mixed",
                "connection-1",
                conversation_id,
                workspace.path(),
                args(
                    vec![
                        DeliverableInput {
                            path: "output/report.pdf".into(),
                            title: Some("Final report".into()),
                            description: None,
                            role: Some("primary".into()),
                            category: Some("standalone_output".into()),
                            change_kind: Some("created".into()),
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
                            category: None,
                            change_kind: None,
                        },
                    ],
                    None,
                ),
            )
            .await;

        assert!(result.published);
        assert_eq!(result.declaration_status, "partial");
        assert_eq!(result.accepted.len(), 1);
        assert_eq!(result.rejected.len(), 1);
        assert!(result.rejected[0].reason.contains("outside"));
        let run = crate::db::entities::conversation_turn_run::Entity::find_by_id("run-1")
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert!(
            run.deliverables_declared_at.is_some(),
            "a partially accepted declaration records successful intent"
        );

        let persisted = deliverable_service::list_for_conversation(&db.conn, conversation_id)
            .await
            .expect("persisted deliverables");
        assert_eq!(persisted.len(), 1);

        let root_result = access
            .publish_deliverables(
                "request-root",
                "connection-1",
                conversation_id,
                workspace.path(),
                args(vec![input(".")], None),
            )
            .await;
        assert!(root_result.accepted.is_empty());
        assert!(root_result.rejected[0].reason.contains("too broad"));

        let result = access
            .publish_deliverables(
                "request-valid",
                "connection-1",
                conversation_id,
                workspace.path(),
                args(
                    vec![DeliverableInput {
                        path: "output/report.pdf".into(),
                        title: Some("Final report".into()),
                        description: None,
                        role: Some("primary".into()),
                        category: Some("standalone_output".into()),
                        change_kind: Some("created".into()),
                    }],
                    None,
                ),
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

        let invalid_remove = access
            .publish_deliverables(
                "request-replace-invalid-remove",
                "connection-1",
                conversation_id,
                workspace.path(),
                PublishDeliverablesArgs {
                    deliverables: vec![DeliverableInput {
                        path: "output/report.pdf".into(),
                        title: Some("Must not persist".into()),
                        description: None,
                        role: Some("primary".into()),
                        category: Some("standalone_output".into()),
                        change_kind: Some("created".into()),
                    }],
                    mode: Some("replace".into()),
                    remove: vec!["../outside.pdf".into()],
                },
            )
            .await;
        assert!(!invalid_remove.published);
        assert_eq!(invalid_remove.declaration_status, "failed");
        assert_eq!(
            deliverable_service::list_for_conversation(&db.conn, conversation_id)
                .await
                .expect("replacement remains atomic")[0]
                .title,
            "Final report"
        );

        let rejected_replacement = access
            .publish_deliverables(
                "request-replace-invalid",
                "connection-1",
                conversation_id,
                workspace.path(),
                args(
                    vec![input(
                        outside
                            .path()
                            .join("secret.txt")
                            .to_string_lossy()
                            .to_string(),
                    )],
                    Some("replace"),
                ),
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
                "request-clear",
                "connection-1",
                conversation_id,
                workspace.path(),
                args(Vec::new(), Some("clear")),
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

    #[tokio::test]
    async fn code_change_publish_is_idempotent_after_the_turn_ends() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(workspace.path().join("src")).expect("source directory");
        std::fs::write(
            workspace.path().join("src/lib.rs"),
            b"pub fn answer() -> u8 { 42 }",
        )
        .expect("source file");
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        artifact_service::create_run(
            &db.conn,
            NewTurnRun {
                id: "run-code".into(),
                conversation_id,
                connection_id: "connection-code".into(),
                client_message_id: Some("message-code".into()),
                prompt_fingerprint: None,
                folder_id: Some(folder_id),
                root_path: std::fs::canonicalize(workspace.path())
                    .expect("canonical workspace")
                    .to_string_lossy()
                    .to_string(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
                expectation_json: r#"{"publish_required":true,"expects_code_changes":true,"requested_paths":["src/lib.rs"]}"#.into(),
            },
        )
        .await
        .expect("active run");
        let access = DbSessionDeliverableAccess::new(db.conn.clone(), EventEmitter::Noop);
        let request = args(
            vec![DeliverableInput {
                path: "src/lib.rs".into(),
                title: Some("Implementation".into()),
                description: None,
                role: None,
                category: Some("code_change".into()),
                change_kind: Some("modified".into()),
            }],
            None,
        );
        let first = access
            .publish_deliverables(
                "request-code",
                "connection-code",
                conversation_id,
                workspace.path(),
                request.clone(),
            )
            .await;
        assert!(first.published);
        assert_eq!(first.declaration_status, "success");
        assert_eq!(first.accepted[0].category, "code_change");
        assert_eq!(first.accepted[0].change_kind, "modified");
        assert_eq!(first.accepted[0].role, "supporting");

        artifact_service::finish_run(
            &db.conn,
            "run-code",
            crate::db::entities::conversation_turn_run::ConversationTurnRunStatus::Completed,
            None,
        )
        .await
        .expect("finish run");

        // The companion retries with the same request id if the first response
        // is lost. The cached outcome must remain available after the active
        // turn has ended and must not create a duplicate association.
        let replay = access
            .publish_deliverables(
                "request-code",
                "connection-code",
                conversation_id,
                workspace.path(),
                request,
            )
            .await;
        assert_eq!(replay, first);
        assert_eq!(
            deliverable_service::list_for_turn(&db.conn, conversation_id, "run-code")
                .await
                .expect("turn deliverables")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn failed_publish_does_not_disable_terminal_code_change_recovery() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::create_dir_all(workspace.path().join("src")).expect("source directory");
        std::fs::write(workspace.path().join("src/lib.rs"), b"pub fn fixed() {}")
            .expect("source file");
        std::fs::write(outside.path().join("secret.rs"), b"secret").expect("outside file");
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        artifact_service::create_run(
            &db.conn,
            NewTurnRun {
                id: "run-failed".into(),
                conversation_id,
                connection_id: "connection-failed".into(),
                client_message_id: Some("message-failed".into()),
                prompt_fingerprint: None,
                folder_id: Some(folder_id),
                root_path: std::fs::canonicalize(workspace.path())
                    .expect("canonical workspace")
                    .to_string_lossy()
                    .to_string(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
                expectation_json: r#"{"publish_required":true,"expects_code_changes":true,"requested_paths":["src/lib.rs"]}"#.into(),
            },
        )
        .await
        .expect("active run");
        let access = DbSessionDeliverableAccess::new(db.conn.clone(), EventEmitter::Noop);
        let failed = access
            .publish_deliverables(
                "request-failed",
                "connection-failed",
                conversation_id,
                workspace.path(),
                args(
                    vec![input(
                        outside
                            .path()
                            .join("secret.rs")
                            .to_string_lossy()
                            .to_string(),
                    )],
                    None,
                ),
            )
            .await;
        assert!(!failed.published);
        assert_eq!(failed.declaration_status, "failed");
        let run = crate::db::entities::conversation_turn_run::Entity::find_by_id("run-failed")
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.declaration_status, "failed");
        assert!(run.declaration_attempted_at.is_some());
        assert!(run.deliverables_declared_at.is_none());

        artifact_service::upsert_changes(
            &db.conn,
            "run-failed",
            vec![crate::db::service::artifact_service::PendingFileChange {
                path: "src/lib.rs".into(),
                kind: crate::db::entities::conversation_turn_file_change::ConversationTurnFileChangeKind::Modified,
                attribution: "exclusive".into(),
            }],
        )
        .await
        .expect("record source change");
        let change = artifact_service::list_changes_for_run(&db.conn, "run-failed")
            .await
            .expect("list changes")
            .pop()
            .expect("source change");
        let metadata = std::fs::metadata(workspace.path().join("src/lib.rs")).unwrap();
        artifact_service::update_final_state(
            &db.conn,
            change,
            true,
            i64::try_from(metadata.len()).ok(),
            metadata
                .modified()
                .ok()
                .map(chrono::DateTime::<chrono::Utc>::from),
        )
        .await
        .expect("finalize source");
        artifact_service::finish_run(
            &db.conn,
            "run-failed",
            crate::db::entities::conversation_turn_run::ConversationTurnRunStatus::Completed,
            None,
        )
        .await
        .expect("finish run");

        let recovered =
            deliverable_service::infer_for_turn(&db.conn, conversation_id, "run-failed")
                .await
                .expect("terminal settlement");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].path, "src/lib.rs");
        assert_eq!(recovered[0].category, "code_change");
        assert_eq!(recovered[0].change_kind, "modified");
    }
}
