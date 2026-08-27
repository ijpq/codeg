use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};

use crate::db::entities::conversation_turn_file_change::ConversationTurnFileChangeKind;
use crate::db::entities::conversation_turn_run::{self, ConversationTurnRunStatus};
use crate::db::entities::{
    conversation, conversation_deliverable, conversation_turn_deliverable,
    conversation_turn_file_change, deliverable_declaration, folder,
};
use crate::db::error::DbError;
use crate::db::service::deliverable_path::deliverable_path_identity;
use crate::models::{
    ConversationDeliverable, ConversationDeliverableHistoryGroup,
    ConversationDeliverableHistoryPage, ConversationTurnDeliverableSet, MessageTurn,
};

pub const SOURCE_DECLARED: &str = "declared";
pub const SOURCE_INFERRED: &str = "inferred";

fn run_has_authoritative_declaration(run: &conversation_turn_run::Model) -> bool {
    matches!(
        run.declaration_status.as_str(),
        "success" | "success_empty" | "partial"
    )
}

/// A successful agent declaration is the user-visible set for that turn.
/// Older releases could append tracker inference after `publish_deliverables`
/// completed; keep those rows as diagnostic history in SQLite, but never mix
/// them back into the declared set returned to clients.
fn retain_authoritative_associations(
    associations: Vec<conversation_turn_deliverable::Model>,
    authoritative_run_ids: &HashSet<String>,
) -> Vec<conversation_turn_deliverable::Model> {
    associations
        .into_iter()
        .filter(|association| {
            !authoritative_run_ids.contains(&association.turn_run_id)
                || association.source == SOURCE_DECLARED
        })
        .collect()
}

/// A declaration that has already passed workspace containment and filesystem
/// validation. Keeping this type internal to the persistence boundary prevents
/// callers from bypassing the verifier with arbitrary paths.
#[derive(Debug, Clone)]
pub struct VerifiedDeliverable {
    pub root_path: String,
    pub path: String,
    pub kind: String,
    pub title: String,
    pub description: Option<String>,
    pub role: String,
    pub category: String,
    pub change_kind: String,
    pub file_name: String,
    pub extension: Option<String>,
    pub size_bytes: Option<i64>,
    pub modified_at: Option<DateTime<Utc>>,
    pub is_valid: bool,
    pub invalid_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedDeliverable {
    pub model: conversation_deliverable::Model,
    pub absolute_path: PathBuf,
}

#[derive(Debug)]
struct InspectedPath {
    absolute_path: PathBuf,
    file_name: String,
    extension: Option<String>,
    size_bytes: Option<i64>,
    modified_at: Option<DateTime<Utc>>,
}

fn clean_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn validate_relative_path(path: &str) -> Result<&Path, DbError> {
    let path = Path::new(path);
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(DbError::Validation(
            "deliverable path must be a non-empty relative path".into(),
        ));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(DbError::Validation(
            "deliverable path contains an unsafe component".into(),
        ));
    }
    Ok(path)
}

/// Re-resolve a persisted path for every read-side file operation. This both
/// reports moved/deleted files and prevents a replaced symlink from escaping
/// the workspace after the original declaration was accepted.
fn inspect_persisted_path(
    root_path: &str,
    relative_path: &str,
    expected_kind: &str,
) -> Result<InspectedPath, DbError> {
    let root = Path::new(root_path);
    if !root.is_absolute() {
        return Err(DbError::Validation(
            "persisted deliverable root is not absolute".into(),
        ));
    }
    let relative = validate_relative_path(relative_path)?;
    let canonical_root = std::fs::canonicalize(root)?;
    let canonical_target = std::fs::canonicalize(canonical_root.join(relative))?;
    if !canonical_target.starts_with(&canonical_root) || canonical_target == canonical_root {
        return Err(DbError::Validation(
            "deliverable resolves outside its workspace".into(),
        ));
    }
    let metadata = std::fs::metadata(&canonical_target)?;
    let actual_kind = if metadata.is_file() {
        "file"
    } else if metadata.is_dir() {
        "directory"
    } else {
        return Err(DbError::Validation(
            "deliverable is not a regular file or directory".into(),
        ));
    };
    if expected_kind != actual_kind {
        return Err(DbError::Validation(format!(
            "deliverable type changed from {expected_kind} to {actual_kind}"
        )));
    }
    let file_name = canonical_target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| DbError::Validation("deliverable has no file name".into()))?;
    Ok(InspectedPath {
        extension: clean_extension(&canonical_target),
        file_name,
        size_bytes: metadata
            .is_file()
            .then(|| i64::try_from(metadata.len()).ok())
            .flatten(),
        modified_at: metadata.modified().ok().map(DateTime::<Utc>::from),
        absolute_path: canonical_target,
    })
}

fn invalid_reason(error: &DbError) -> String {
    match error {
        DbError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
            "file_not_found".to_string()
        }
        DbError::Io(_) => "file_unavailable".to_string(),
        DbError::Validation(_) => "unsafe_or_changed_path".to_string(),
        _ => "validation_failed".to_string(),
    }
}

fn to_info(
    model: conversation_deliverable::Model,
    association: Option<&conversation_turn_deliverable::Model>,
    run: Option<&conversation_turn_run::Model>,
) -> ConversationDeliverable {
    let source = association
        .map(|row| row.source.clone())
        .unwrap_or_else(|| model.source.clone());
    let role = association
        .map(|row| row.role.clone())
        .unwrap_or_else(|| model.role.clone());
    let category = association
        .map(|row| row.category.clone())
        .unwrap_or_else(|| model.category.clone());
    let change_kind = association
        .map(|row| row.change_kind.clone())
        .unwrap_or_else(|| model.change_kind.clone());
    let position = association
        .map(|row| row.position)
        .unwrap_or(model.position);
    let title = association
        .map(|row| row.title.clone())
        .unwrap_or_else(|| model.title.clone());
    let description = association
        .map(|row| row.description.clone())
        .unwrap_or_else(|| model.description.clone());
    let produced_at = association
        .map(|row| row.created_at)
        .unwrap_or(model.created_at);
    ConversationDeliverable {
        id: model.id,
        conversation_id: model.conversation_id,
        turn_run_id: run
            .map(|row| row.id.clone())
            .or_else(|| model.turn_run_id.clone()),
        root_path: model.root_path,
        path: model.path,
        kind: model.kind,
        title,
        description,
        role,
        category,
        change_kind,
        position,
        source,
        file_name: model.file_name,
        extension: model.extension,
        size_bytes: model.size_bytes,
        modified_at: model.modified_at,
        is_valid: model.is_valid,
        invalid_reason: model.invalid_reason,
        verified_at: model.verified_at,
        last_checked_at: model.last_checked_at,
        turn_client_message_id: run.and_then(|row| row.client_message_id.clone()),
        turn_started_at: run.map(|row| row.started_at),
        produced_at,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

pub async fn active_turn_run(
    conn: &DatabaseConnection,
    conversation_id: i32,
    connection_id: &str,
) -> Result<Option<conversation_turn_run::Model>, DbError> {
    Ok(conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .filter(conversation_turn_run::Column::ConnectionId.eq(connection_id))
        .filter(conversation_turn_run::Column::Status.eq(ConversationTurnRunStatus::Running))
        .order_by_desc(conversation_turn_run::Column::StartedAt)
        .one(conn)
        .await?)
}

pub async fn active_turn_run_id(
    conn: &DatabaseConnection,
    conversation_id: i32,
    connection_id: &str,
) -> Result<Option<String>, DbError> {
    Ok(active_turn_run(conn, conversation_id, connection_id)
        .await?
        .map(|run| run.id))
}

pub async fn mark_declaration_result(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turn_run_id: &str,
    status: &str,
    error: Option<String>,
) -> Result<bool, DbError> {
    let Some(run) = conversation_turn_run::Entity::find_by_id(turn_run_id.to_string())
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .filter(conversation_turn_run::Column::Status.eq(ConversationTurnRunStatus::Running))
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    let now = Utc::now();
    let mut active = run.into_active_model();
    active.declaration_status = Set(status.to_string());
    active.declaration_attempted_at = Set(Some(now));
    active.declaration_error = Set(error);
    if matches!(status, "success" | "success_empty" | "partial") {
        active.deliverables_declared_at = Set(Some(now));
    }
    active.update(conn).await?;
    Ok(true)
}

pub async fn cached_declaration_outcome(
    conn: &DatabaseConnection,
    request_id: &str,
    conversation_id: i32,
) -> Result<Option<String>, DbError> {
    Ok(
        deliverable_declaration::Entity::find_by_id(request_id.to_string())
            .filter(deliverable_declaration::Column::ConversationId.eq(conversation_id))
            .one(conn)
            .await?
            .map(|row| row.outcome_json),
    )
}

pub async fn cache_declaration_outcome(
    conn: &DatabaseConnection,
    request_id: &str,
    conversation_id: i32,
    turn_run_id: &str,
    status: &str,
    payload_json: &str,
    outcome_json: &str,
) -> Result<(), DbError> {
    let now = Utc::now();
    if let Some(existing) = deliverable_declaration::Entity::find_by_id(request_id.to_string())
        .one(conn)
        .await?
    {
        let mut active = existing.into_active_model();
        active.status = Set(status.to_string());
        active.payload_json = Set(payload_json.to_string());
        active.outcome_json = Set(outcome_json.to_string());
        active.updated_at = Set(now);
        active.update(conn).await?;
    } else {
        deliverable_declaration::ActiveModel {
            request_id: Set(request_id.to_string()),
            conversation_id: Set(conversation_id),
            turn_run_id: Set(turn_run_id.to_string()),
            status: Set(status.to_string()),
            payload_json: Set(payload_json.to_string()),
            outcome_json: Set(outcome_json.to_string()),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(conn)
        .await?;
    }
    Ok(())
}

async fn save_turn_set(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turn_run_id: &str,
    source: &str,
    items: Vec<VerifiedDeliverable>,
    require_running: bool,
    replace_existing: bool,
) -> Result<Vec<ConversationDeliverable>, DbError> {
    if source != SOURCE_DECLARED && source != SOURCE_INFERRED {
        return Err(DbError::Validation("invalid deliverable source".into()));
    }
    let txn = conn.begin().await?;
    if conversation::Entity::find_by_id(conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(&txn)
        .await?
        .is_none()
    {
        return Err(DbError::NotFound(format!("conversation {conversation_id}")));
    }
    let Some(run) = conversation_turn_run::Entity::find_by_id(turn_run_id.to_string())
        .one(&txn)
        .await?
    else {
        return Err(DbError::NotFound(format!("turn run {turn_run_id}")));
    };
    if run.conversation_id != conversation_id {
        return Err(DbError::Validation(
            "turn run does not belong to the conversation".into(),
        ));
    }
    if require_running && run.status != ConversationTurnRunStatus::Running {
        return Err(DbError::Validation("the active turn already ended".into()));
    }
    if !require_running && run.status == ConversationTurnRunStatus::Running {
        return Err(DbError::Validation(
            "fallback inference is only allowed for terminal turns".into(),
        ));
    }

    let now = Utc::now();
    let previous = conversation_turn_deliverable::Entity::find()
        .filter(conversation_turn_deliverable::Column::TurnRunId.eq(turn_run_id.to_string()))
        .all(&txn)
        .await?;
    let previous_ids = previous
        .iter()
        .map(|row| row.deliverable_id.clone())
        .collect::<HashSet<_>>();
    let position_offset = if replace_existing {
        conversation_turn_deliverable::Entity::delete_many()
            .filter(conversation_turn_deliverable::Column::TurnRunId.eq(turn_run_id.to_string()))
            .exec(&txn)
            .await?;
        0
    } else {
        i32::try_from(previous.len()).unwrap_or(i32::MAX)
    };

    // The legacy unique index compares raw strings, which is not a valid file
    // identity on Windows (`D:/x` and `d:\\x` are the same place). Build the
    // lookup once from all existing rows so new declarations reuse a durable
    // identity even when an old row uses another slash or drive-letter style.
    // New rows use the canonical forward-slash storage form; existing rows are
    // not rewritten in place because an old database may already contain two
    // logical twins whose raw keys differ.
    let mut known_by_path = HashMap::new();
    for model in conversation_deliverable::Entity::find()
        .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
        .order_by_desc(conversation_deliverable::Column::UpdatedAt)
        .all(&txn)
        .await?
    {
        known_by_path
            .entry(deliverable_path_identity(&model.root_path, &model.path).identity)
            .or_insert(model);
    }

    let mut saved_pairs = Vec::with_capacity(items.len());
    let mut retained_ids = HashSet::new();
    for (position, item) in items.into_iter().enumerate() {
        let position = position_offset.saturating_add(i32::try_from(position).unwrap_or(i32::MAX));
        let normalized_path = deliverable_path_identity(&item.root_path, &item.path);
        let existing = known_by_path.get(&normalized_path.identity).cloned();

        let model = if let Some(existing) = existing {
            let mut active = existing.into_active_model();
            active.turn_run_id = Set(Some(turn_run_id.to_string()));
            active.kind = Set(item.kind.clone());
            active.title = Set(item.title.clone());
            active.description = Set(item.description.clone());
            active.role = Set(item.role.clone());
            active.category = Set(item.category.clone());
            active.change_kind = Set(item.change_kind.clone());
            active.position = Set(position);
            active.source = Set(source.to_string());
            active.file_name = Set(item.file_name.clone());
            active.extension = Set(item.extension.clone());
            active.size_bytes = Set(item.size_bytes);
            active.modified_at = Set(item.modified_at);
            active.is_valid = Set(item.is_valid);
            active.invalid_reason = Set(item.invalid_reason.clone());
            active.is_hidden = Set(false);
            active.verified_at = Set(now);
            active.last_checked_at = Set(Some(now));
            active.updated_at = Set(now);
            active.update(&txn).await?
        } else {
            conversation_deliverable::ActiveModel {
                id: Set(uuid::Uuid::new_v4().to_string()),
                conversation_id: Set(conversation_id),
                turn_run_id: Set(Some(turn_run_id.to_string())),
                root_path: Set(normalized_path.storage_root),
                path: Set(normalized_path.storage_path),
                kind: Set(item.kind),
                title: Set(item.title.clone()),
                description: Set(item.description.clone()),
                role: Set(item.role.clone()),
                category: Set(item.category.clone()),
                change_kind: Set(item.change_kind.clone()),
                position: Set(position),
                source: Set(source.to_string()),
                file_name: Set(item.file_name),
                extension: Set(item.extension),
                size_bytes: Set(item.size_bytes),
                modified_at: Set(item.modified_at),
                is_valid: Set(item.is_valid),
                invalid_reason: Set(item.invalid_reason.clone()),
                is_hidden: Set(false),
                verified_at: Set(now),
                last_checked_at: Set(Some(now)),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&txn)
            .await?
        };
        known_by_path.insert(normalized_path.identity, model.clone());
        retained_ids.insert(model.id.clone());
        let existing_association = if replace_existing {
            None
        } else {
            conversation_turn_deliverable::Entity::find()
                .filter(
                    conversation_turn_deliverable::Column::TurnRunId.eq(turn_run_id.to_string()),
                )
                .filter(conversation_turn_deliverable::Column::DeliverableId.eq(model.id.clone()))
                .one(&txn)
                .await?
        };
        let association = if let Some(existing_association) = existing_association {
            let mut active = existing_association.into_active_model();
            active.source = Set(source.to_string());
            active.title = Set(item.title);
            active.description = Set(item.description);
            active.role = Set(item.role);
            active.category = Set(item.category);
            active.change_kind = Set(item.change_kind);
            active.updated_at = Set(now);
            active.update(&txn).await?
        } else {
            conversation_turn_deliverable::ActiveModel {
                id: Set(uuid::Uuid::new_v4().to_string()),
                conversation_id: Set(conversation_id),
                turn_run_id: Set(turn_run_id.to_string()),
                deliverable_id: Set(model.id.clone()),
                source: Set(source.to_string()),
                title: Set(item.title),
                description: Set(item.description),
                role: Set(item.role),
                category: Set(item.category),
                change_kind: Set(item.change_kind),
                position: Set(position),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&txn)
            .await?
        };
        saved_pairs.push((model, association));
    }

    // A second declaration replaces only this turn. Remove aggregate rows that
    // no longer have any turn history; otherwise restore their most recent
    // remaining association as the aggregate's latest provenance.
    for deliverable_id in previous_ids
        .difference(&retained_ids)
        .filter(|_| replace_existing)
    {
        let latest = conversation_turn_deliverable::Entity::find()
            .filter(conversation_turn_deliverable::Column::DeliverableId.eq(deliverable_id.clone()))
            .order_by_desc(conversation_turn_deliverable::Column::CreatedAt)
            .one(&txn)
            .await?;
        if let Some(latest) = latest {
            if let Some(model) =
                conversation_deliverable::Entity::find_by_id(deliverable_id.clone())
                    .one(&txn)
                    .await?
            {
                let mut active = model.into_active_model();
                active.turn_run_id = Set(Some(latest.turn_run_id));
                active.source = Set(latest.source);
                active.title = Set(latest.title);
                active.description = Set(latest.description);
                active.role = Set(latest.role);
                active.category = Set(latest.category);
                active.change_kind = Set(latest.change_kind);
                active.position = Set(latest.position);
                active.updated_at = Set(now);
                active.update(&txn).await?;
            }
        } else {
            conversation_deliverable::Entity::delete_by_id(deliverable_id.clone())
                .exec(&txn)
                .await?;
        }
    }

    txn.commit().await?;
    Ok(saved_pairs
        .into_iter()
        .map(|(model, association)| to_info(model, Some(&association), Some(&run)))
        .collect())
}

pub async fn save_declared_for_turn(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turn_run_id: &str,
    items: Vec<VerifiedDeliverable>,
    replace_existing: bool,
) -> Result<Vec<ConversationDeliverable>, DbError> {
    save_turn_set(
        conn,
        conversation_id,
        turn_run_id,
        SOURCE_DECLARED,
        items,
        true,
        replace_existing,
    )
    .await
}

/// Compatibility helper for callers that intentionally submit a complete set.
pub async fn replace_declared_for_turn(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turn_run_id: &str,
    items: Vec<VerifiedDeliverable>,
) -> Result<Vec<ConversationDeliverable>, DbError> {
    save_declared_for_turn(conn, conversation_id, turn_run_id, items, true).await
}

pub async fn remove_declared_paths_for_turn(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turn_run_id: &str,
    root_path: &str,
    paths: &[String],
) -> Result<(), DbError> {
    if paths.is_empty() {
        return Ok(());
    }
    let txn = conn.begin().await?;
    let Some(run) = conversation_turn_run::Entity::find_by_id(turn_run_id.to_string())
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .filter(conversation_turn_run::Column::Status.eq(ConversationTurnRunStatus::Running))
        .one(&txn)
        .await?
    else {
        return Err(DbError::Validation("the active turn already ended".into()));
    };
    let models = conversation_deliverable::Entity::find()
        .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
        .filter(conversation_deliverable::Column::RootPath.eq(root_path.to_string()))
        .filter(conversation_deliverable::Column::Path.is_in(paths.to_vec()))
        .all(&txn)
        .await?;
    let ids = models.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
    if ids.is_empty() {
        txn.commit().await?;
        return Ok(());
    }
    conversation_turn_deliverable::Entity::delete_many()
        .filter(conversation_turn_deliverable::Column::TurnRunId.eq(turn_run_id.to_string()))
        .filter(conversation_turn_deliverable::Column::DeliverableId.is_in(ids.clone()))
        .exec(&txn)
        .await?;
    let now = Utc::now();
    for deliverable_id in ids {
        let latest = conversation_turn_deliverable::Entity::find()
            .filter(conversation_turn_deliverable::Column::DeliverableId.eq(deliverable_id.clone()))
            .order_by_desc(conversation_turn_deliverable::Column::CreatedAt)
            .one(&txn)
            .await?;
        if let Some(latest) = latest {
            if let Some(model) =
                conversation_deliverable::Entity::find_by_id(deliverable_id.clone())
                    .one(&txn)
                    .await?
            {
                let mut active = model.into_active_model();
                active.turn_run_id = Set(Some(latest.turn_run_id));
                active.source = Set(latest.source);
                active.title = Set(latest.title);
                active.description = Set(latest.description);
                active.role = Set(latest.role);
                active.category = Set(latest.category);
                active.change_kind = Set(latest.change_kind);
                active.position = Set(latest.position);
                active.updated_at = Set(now);
                active.update(&txn).await?;
            }
        } else {
            conversation_deliverable::Entity::delete_by_id(deliverable_id)
                .exec(&txn)
                .await?;
        }
    }
    let _ = run;
    txn.commit().await?;
    Ok(())
}

fn standalone_output_extension(extension: Option<&str>) -> bool {
    matches!(
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
    )
}

fn inference_path_hard_allowed(path: &str) -> bool {
    let Ok(parsed) = validate_relative_path(path) else {
        return false;
    };
    const EXCLUDED_COMPONENTS: &[&str] = &[
        ".git",
        ".next",
        ".turbo",
        ".cache",
        ".codex",
        ".claude",
        "node_modules",
        "target",
        "coverage",
        "logs",
        "log",
        "cache",
        "caches",
        "tmp",
        "temp",
        "_qa",
        "qa",
        "_rendered",
        "rendered",
        "_backups",
        "backup",
        "backups",
        "__pycache__",
    ];
    if parsed.components().any(|component| match component {
        Component::Normal(name) => EXCLUDED_COMPONENTS
            .iter()
            .any(|excluded| name.to_string_lossy().eq_ignore_ascii_case(excluded)),
        _ => false,
    }) {
        return false;
    }
    true
}

pub(crate) fn inference_path_allowed(path: &str) -> bool {
    if !inference_path_hard_allowed(path) {
        return false;
    }
    let Ok(parsed) = validate_relative_path(path) else {
        return false;
    };
    let file_name = parsed
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let stem = parsed
        .file_stem()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    const TRANSIENT_HIDDEN_FILES: &[&str] = &[".ds_store", ".eslintcache", ".stylelintcache"];
    if TRANSIENT_HIDDEN_FILES.contains(&file_name.as_str()) || file_name.starts_with("~$") {
        return false;
    }
    if file_name == "status.md" || stem == "_probe" || stem.starts_with("_probe.") {
        return false;
    }
    let extension = clean_extension(parsed);
    if matches!(
        extension.as_deref(),
        Some(
            "py" | "pyc"
                | "ps1"
                | "cmd"
                | "bat"
                | "log"
                | "lock"
                | "tmp"
                | "temp"
                | "cache"
                | "bak"
                | "old"
                | "orig"
        )
    ) {
        return false;
    }
    if matches!(extension.as_deref(), Some("png" | "jpg" | "jpeg" | "webp")) {
        let suffix = stem
            .strip_prefix("page-")
            .or_else(|| stem.strip_prefix("page_"));
        if suffix
            .is_some_and(|value| value.len() >= 2 && value.chars().all(|ch| ch.is_ascii_digit()))
        {
            return false;
        }
    }
    const TRANSIENT_STEMS: &[&str] = &["draft", "preview", "temp", "tmp"];
    if TRANSIENT_STEMS.contains(&stem.as_str()) {
        return false;
    }
    // Test/spec/snapshot source files are real code changes and must survive
    // fallback settlement. Test-like names are filtered only for standalone
    // output formats, where they conventionally identify preview fixtures.
    if standalone_output_extension(extension.as_deref()) {
        const EXCLUDED_OUTPUT_DIRS: &[&str] = &[
            "qa",
            "_qa",
            "qa-images",
            "qa_images",
            "rendered",
            "_rendered",
            "backup",
            "backups",
            "_backups",
            "render-cache",
            "render_cache",
            "render-check",
            "render_check",
        ];
        if parsed.parent().is_some_and(|parent| {
            parent.components().any(|component| match component {
                Component::Normal(name) => EXCLUDED_OUTPUT_DIRS
                    .iter()
                    .any(|excluded| name.to_string_lossy().eq_ignore_ascii_case(excluded)),
                _ => false,
            })
        }) {
            return false;
        }
        const EXCLUDED_STEMS: &[&str] = &["test"];
        if EXCLUDED_STEMS.contains(&stem.as_str()) {
            return false;
        }
        const EXCLUDED_MARKERS: &[&str] = &[
            ".test",
            "_test",
            "-test",
            ".spec",
            "_spec",
            "-spec",
            ".tmp",
            "_tmp",
            "-tmp",
            "draft-",
            "draft_",
            "preview-",
            "preview_",
            "backup-",
            "backup_",
            "old-",
            "old_",
            "qa-",
            "qa_",
            "render-check-",
            "render_check_",
            "visual-check-",
            "visual_check_",
            ".backup",
            "_backup",
            "-backup",
            ".old",
            "_old",
            "-old",
        ];
        if EXCLUDED_MARKERS
            .iter()
            .any(|marker| stem.starts_with(marker) || stem.ends_with(marker))
        {
            return false;
        }
    }
    true
}

/// Deterministically settle one terminal turn. A successful explicit
/// declaration (including an intentional empty declaration) is authoritative;
/// tracker inference is a fallback only when declaration never succeeded.
/// Failed declarations still permit recovery. This intentionally includes
/// source/test/config edits for inference-only turns: code changes are useful in
/// the inclusive conversation ledger, but never supplement a declared set.
pub async fn infer_for_turn(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turn_run_id: &str,
) -> Result<Vec<ConversationDeliverable>, DbError> {
    let Some(run) = conversation_turn_run::Entity::find_by_id(turn_run_id.to_string())
        .one(conn)
        .await?
    else {
        return Ok(Vec::new());
    };
    if run.conversation_id != conversation_id || run.status == ConversationTurnRunStatus::Running {
        return Ok(Vec::new());
    }
    let existing_associations = conversation_turn_deliverable::Entity::find()
        .filter(conversation_turn_deliverable::Column::TurnRunId.eq(turn_run_id.to_string()))
        .all(conn)
        .await?;
    let has_authoritative_declaration = run_has_authoritative_declaration(&run)
        || existing_associations
            .iter()
            .any(|association| association.source == SOURCE_DECLARED);

    let changes = conversation_turn_file_change::Entity::find()
        .filter(conversation_turn_file_change::Column::TurnRunId.eq(turn_run_id.to_string()))
        .order_by_asc(conversation_turn_file_change::Column::FirstSeenAt)
        .all(conn)
        .await?;
    let existing_ids = existing_associations
        .iter()
        .map(|association| association.deliverable_id.clone())
        .collect::<Vec<_>>();
    let existing_paths = if existing_ids.is_empty() {
        HashSet::new()
    } else {
        conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::Id.is_in(existing_ids))
            .all(conn)
            .await?
            .into_iter()
            .map(|row| row.path)
            .collect::<HashSet<_>>()
    };
    let expectation = serde_json::from_str::<crate::artifact_tracker::TurnDeliverableExpectation>(
        &run.expectation_json,
    )
    .unwrap_or_default();
    let inputs = serde_json::from_str::<Vec<String>>(&run.input_paths_json)
        .unwrap_or_default()
        .into_iter()
        .map(|path| path.replace('\\', "/"))
        .collect::<HashSet<_>>();
    let expected_paths = expectation
        .requested_paths
        .iter()
        .cloned()
        .collect::<HashSet<_>>();

    let mut inferred = Vec::new();
    let mut observed_paths = HashSet::new();
    for change in &changes {
        observed_paths.insert(change.path.clone());
        if has_authoritative_declaration {
            continue;
        }
        let explicitly_expected = expected_paths.contains(&change.path);
        let extension = clean_extension(Path::new(&change.path));
        // Preserve source changes in the inclusive internal ledger for failed
        // declaration recovery, but expose only filtered standalone outputs as
        // user-facing inferred deliverables (see is_user_visible_deliverable).
        let ambiguous_standalone_candidate = change.kind != ConversationTurnFileChangeKind::Deleted
            && standalone_output_extension(extension.as_deref());
        let expected_code_recovery = expectation.expects_code_changes
            && explicitly_expected
            && !ambiguous_standalone_candidate;
        if !matches!(
            change.source.as_str(),
            "watcher" | "agent_file_change_report"
        ) || (change.attribution != "exclusive"
            && !explicitly_expected
            && !ambiguous_standalone_candidate)
            || existing_paths.contains(&change.path)
            || (inputs.contains(&change.path)
                && change.kind == ConversationTurnFileChangeKind::Created)
            || !inference_path_hard_allowed(&change.path)
            || (!ambiguous_standalone_candidate && !expected_code_recovery)
            || (ambiguous_standalone_candidate && !inference_path_allowed(&change.path))
        {
            continue;
        }
        let is_deleted = change.kind == ConversationTurnFileChangeKind::Deleted
            && change.final_exists == Some(false);
        if !is_deleted && change.final_exists != Some(true) {
            continue;
        }
        if !matches!(
            change.kind,
            ConversationTurnFileChangeKind::Created
                | ConversationTurnFileChangeKind::Modified
                | ConversationTurnFileChangeKind::Renamed
                | ConversationTurnFileChangeKind::Deleted
        ) {
            continue;
        }
        let (file_name, extension, size_bytes, modified_at, is_valid, invalid_reason) =
            if is_deleted {
                (
                    Path::new(&change.path)
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| change.path.clone()),
                    extension,
                    None,
                    None,
                    false,
                    Some("deleted".to_string()),
                )
            } else {
                let Ok(inspected) = inspect_persisted_path(&run.root_path, &change.path, "file")
                else {
                    continue;
                };
                (
                    inspected.file_name,
                    inspected.extension,
                    inspected.size_bytes,
                    inspected.modified_at,
                    true,
                    None,
                )
            };
        let category = if !is_deleted && standalone_output_extension(extension.as_deref()) {
            "standalone_output"
        } else {
            "code_change"
        };
        inferred.push(VerifiedDeliverable {
            root_path: run.root_path.clone(),
            path: change.path.clone(),
            kind: "file".into(),
            title: file_name.clone(),
            description: None,
            // Role promotion happens only after the complete candidate set is
            // known. This prevents a directory full of page previews from
            // turning into dozens of inferred primary outputs.
            role: "supporting".into(),
            category: category.into(),
            change_kind: match change.kind {
                ConversationTurnFileChangeKind::Created => "created",
                ConversationTurnFileChangeKind::Modified => "modified",
                ConversationTurnFileChangeKind::Deleted => "deleted",
                ConversationTurnFileChangeKind::Renamed => "renamed",
            }
            .into(),
            file_name,
            extension,
            size_bytes,
            modified_at,
            is_valid,
            invalid_reason,
        });
    }

    let expected_candidates = inferred
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.category == "standalone_output" && expected_paths.contains(&item.path)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let primary_index = if expected_candidates.len() == 1 {
        expected_candidates.first().copied()
    } else if inferred
        .iter()
        .filter(|item| item.category == "standalone_output")
        .count()
        == 1
    {
        inferred
            .iter()
            .position(|item| item.category == "standalone_output")
    } else {
        None
    };
    if let Some(index) = primary_index {
        inferred[index].role = "primary".into();
    }

    let saved = if inferred.is_empty() {
        Vec::new()
    } else {
        save_turn_set(
            conn,
            conversation_id,
            turn_run_id,
            SOURCE_INFERRED,
            inferred,
            false,
            false,
        )
        .await?
    };
    let mut missing_expected_paths = expected_paths
        .difference(&observed_paths)
        .filter(|path| !existing_paths.contains(*path))
        .cloned()
        .collect::<Vec<_>>();
    missing_expected_paths.sort();
    crate::db::service::artifact_service::mark_settled(
        conn,
        turn_run_id,
        if run.capture_incomplete || !missing_expected_paths.is_empty() {
            "settled_incomplete"
        } else {
            "settled"
        },
        &missing_expected_paths,
    )
    .await?;
    Ok(saved)
}

async fn refreshed_models_for_ids(
    conn: &DatabaseConnection,
    conversation_id: i32,
    ids: Option<&HashSet<String>>,
) -> Result<Vec<conversation_deliverable::Model>, DbError> {
    if ids.is_some_and(HashSet::is_empty) {
        return Ok(Vec::new());
    }
    let mut query = conversation_deliverable::Entity::find()
        .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
        .filter(conversation_deliverable::Column::IsHidden.eq(false))
        .order_by_desc(conversation_deliverable::Column::UpdatedAt);
    if let Some(ids) = ids {
        query = query.filter(conversation_deliverable::Column::Id.is_in(ids.iter().cloned()));
    }
    let rows = query.all(conn).await?;
    let mut refreshed = Vec::with_capacity(rows.len());
    for row in rows {
        let now = Utc::now();
        if row.change_kind == "deleted" {
            let fallback_extension = clean_extension(Path::new(&row.path));
            let fallback_file_name = Path::new(&row.path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| row.title.clone());
            let mut active = row.into_active_model();
            active.file_name = Set(fallback_file_name);
            active.extension = Set(fallback_extension);
            active.size_bytes = Set(None);
            active.modified_at = Set(None);
            active.is_valid = Set(false);
            active.invalid_reason = Set(Some("deleted".to_string()));
            active.last_checked_at = Set(Some(now));
            refreshed.push(active.update(conn).await?);
            continue;
        }
        let inspection = inspect_persisted_path(&row.root_path, &row.path, &row.kind);
        let fallback_file_name = Path::new(&row.path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| row.title.clone());
        let fallback_extension = clean_extension(Path::new(&row.path));
        let mut active = row.into_active_model();
        match inspection {
            Ok(info) => {
                active.file_name = Set(info.file_name);
                active.extension = Set(info.extension);
                active.size_bytes = Set(info.size_bytes);
                active.modified_at = Set(info.modified_at);
                active.is_valid = Set(true);
                active.invalid_reason = Set(None);
            }
            Err(error) => {
                active.file_name = Set(fallback_file_name);
                active.extension = Set(fallback_extension);
                active.is_valid = Set(false);
                active.invalid_reason = Set(Some(invalid_reason(&error)));
            }
        }
        active.last_checked_at = Set(Some(now));
        refreshed.push(active.update(conn).await?);
    }
    Ok(refreshed)
}

async fn refreshed_models(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<conversation_deliverable::Model>, DbError> {
    refreshed_models_for_ids(conn, conversation_id, None).await
}

fn normalized_deliverable_path_key(root_path: &str, path: &str) -> String {
    deliverable_path_identity(root_path, path).identity
}

fn is_user_visible_deliverable(item: &ConversationDeliverable) -> bool {
    if !item.is_valid || item.change_kind == "deleted" {
        return false;
    }
    item.source == SOURCE_DECLARED
        || (item.source == SOURCE_INFERRED
            && item.category == "standalone_output"
            && inference_path_allowed(&item.path))
}

async fn history_groups_for_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
    refresh_filesystem: bool,
) -> Result<Vec<ConversationDeliverableHistoryGroup>, DbError> {
    let models = if refresh_filesystem {
        refreshed_models(conn, conversation_id).await?
    } else {
        // History opens frequently and is paged for display. Trust the
        // persisted validation snapshot here instead of statting every file in
        // a long conversation before returning page one. Access operations and
        // the current-turn detail path still revalidate their bounded id set.
        conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
            .filter(conversation_deliverable::Column::IsHidden.eq(false))
            .filter(conversation_deliverable::Column::IsValid.eq(true))
            .order_by_desc(conversation_deliverable::Column::UpdatedAt)
            .all(conn)
            .await?
    };
    if models.is_empty() {
        return Ok(Vec::new());
    }
    let model_map = models
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect::<HashMap<_, _>>();
    let associations = conversation_turn_deliverable::Entity::find()
        .filter(conversation_turn_deliverable::Column::ConversationId.eq(conversation_id))
        .all(conn)
        .await?;
    let run_ids = associations
        .iter()
        .map(|row| row.turn_run_id.clone())
        .collect::<HashSet<_>>();
    let runs = if run_ids.is_empty() {
        Vec::new()
    } else {
        conversation_turn_run::Entity::find()
            .filter(conversation_turn_run::Column::Id.is_in(run_ids))
            .all(conn)
            .await?
    };
    let authoritative_run_ids = runs
        .iter()
        .filter(|run| run_has_authoritative_declaration(run))
        .map(|run| run.id.clone())
        .chain(
            associations
                .iter()
                .filter(|association| association.source == SOURCE_DECLARED)
                .map(|association| association.turn_run_id.clone()),
        )
        .collect::<HashSet<_>>();
    let associations = retain_authoritative_associations(associations, &authoritative_run_ids);
    let run_map = runs
        .into_iter()
        .map(|run| (run.id.clone(), run))
        .collect::<HashMap<_, _>>();
    let mut grouped: HashMap<String, Vec<ConversationDeliverable>> = HashMap::new();
    for association in associations {
        let Some(model) = model_map.get(&association.deliverable_id).cloned() else {
            continue;
        };
        let run = run_map.get(&association.turn_run_id);
        let item = to_info(model, Some(&association), run);
        if !is_user_visible_deliverable(&item) {
            continue;
        }
        let key = normalized_deliverable_path_key(&item.root_path, &item.path);
        grouped.entry(key).or_default().push(item);
    }

    let mut groups = grouped
        .into_iter()
        .filter_map(|(path_key, mut versions)| {
            versions.sort_by(|left, right| {
                right
                    .produced_at
                    .cmp(&left.produced_at)
                    .then_with(|| right.updated_at.cmp(&left.updated_at))
            });
            versions.dedup_by(|left, right| {
                left.turn_run_id == right.turn_run_id && left.source == right.source
            });
            // Authoritative suppression above resolves declared/inferred
            // duplicates within one run. Across runs this must remain the
            // newest valid version, even when an older version was declared.
            let latest = versions.first()?.clone();
            Some(ConversationDeliverableHistoryGroup {
                path_key,
                latest,
                versions,
            })
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        let left_time = left
            .versions
            .first()
            .map(|item| item.produced_at)
            .unwrap_or(left.latest.produced_at);
        let right_time = right
            .versions
            .first()
            .map(|item| item.produced_at)
            .unwrap_or(right.latest.produced_at);
        right_time.cmp(&left_time)
    });
    Ok(groups)
}

pub async fn list_history_page(
    conn: &DatabaseConnection,
    conversation_id: i32,
    offset: u32,
    limit: u32,
) -> Result<ConversationDeliverableHistoryPage, DbError> {
    let groups = history_groups_for_conversation(conn, conversation_id, false).await?;
    let total = u32::try_from(groups.len()).unwrap_or(u32::MAX);
    let start = usize::try_from(offset)
        .unwrap_or(usize::MAX)
        .min(groups.len());
    let limit = usize::try_from(limit.clamp(1, 100)).unwrap_or(25);
    let end = start.saturating_add(limit).min(groups.len());
    let items = groups[start..end].to_vec();
    let has_more = end < groups.len();
    Ok(ConversationDeliverableHistoryPage {
        items,
        offset,
        next_offset: has_more.then(|| u32::try_from(end).unwrap_or(u32::MAX)),
        has_more,
        total,
    })
}

pub async fn list_for_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<ConversationDeliverable>, DbError> {
    Ok(history_groups_for_conversation(conn, conversation_id, true)
        .await?
        .into_iter()
        .map(|group| group.latest)
        .collect())
}

pub async fn list_for_turn(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turn_run_id: &str,
) -> Result<Vec<ConversationDeliverable>, DbError> {
    let models = refreshed_models(conn, conversation_id).await?;
    let model_map = models
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect::<HashMap<_, _>>();
    let run = conversation_turn_run::Entity::find_by_id(turn_run_id.to_string())
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .one(conn)
        .await?;
    let Some(run) = run else {
        return Ok(Vec::new());
    };
    let associations = conversation_turn_deliverable::Entity::find()
        .filter(conversation_turn_deliverable::Column::ConversationId.eq(conversation_id))
        .filter(conversation_turn_deliverable::Column::TurnRunId.eq(turn_run_id.to_string()))
        .order_by_asc(conversation_turn_deliverable::Column::Position)
        .all(conn)
        .await?;
    let authoritative_run_ids = if run_has_authoritative_declaration(&run)
        || associations
            .iter()
            .any(|association| association.source == SOURCE_DECLARED)
    {
        HashSet::from([run.id.clone()])
    } else {
        HashSet::new()
    };
    let associations = retain_authoritative_associations(associations, &authoritative_run_ids);
    Ok(associations
        .into_iter()
        .filter_map(|association| {
            model_map
                .get(&association.deliverable_id)
                .cloned()
                .map(|model| to_info(model, Some(&association), Some(&run)))
                .filter(is_user_visible_deliverable)
        })
        .collect())
}

async fn build_sets_for_runs(
    conn: &DatabaseConnection,
    conversation_id: i32,
    runs: Vec<conversation_turn_run::Model>,
) -> Result<Vec<ConversationTurnDeliverableSet>, DbError> {
    if runs.is_empty() {
        return Ok(Vec::new());
    }
    let run_ids = runs.iter().map(|run| run.id.clone()).collect::<Vec<_>>();
    let mut associations = Vec::new();
    for run_id_chunk in run_ids.chunks(500) {
        associations.extend(
            conversation_turn_deliverable::Entity::find()
                .filter(conversation_turn_deliverable::Column::ConversationId.eq(conversation_id))
                .filter(
                    conversation_turn_deliverable::Column::TurnRunId.is_in(run_id_chunk.to_vec()),
                )
                .order_by_asc(conversation_turn_deliverable::Column::Position)
                .all(conn)
                .await?,
        );
    }
    let deliverable_ids = associations
        .iter()
        .map(|association| association.deliverable_id.clone())
        .collect::<HashSet<_>>();
    let models = refreshed_models_for_ids(conn, conversation_id, Some(&deliverable_ids)).await?;
    let model_map = models
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect::<HashMap<_, _>>();
    let authoritative_run_ids = runs
        .iter()
        .filter(|run| run_has_authoritative_declaration(run))
        .map(|run| run.id.clone())
        .chain(
            associations
                .iter()
                .filter(|association| association.source == SOURCE_DECLARED)
                .map(|association| association.turn_run_id.clone()),
        )
        .collect::<HashSet<_>>();
    let associations = retain_authoritative_associations(associations, &authoritative_run_ids);
    let mut by_run: HashMap<String, Vec<conversation_turn_deliverable::Model>> = HashMap::new();
    for association in associations {
        by_run
            .entry(association.turn_run_id.clone())
            .or_default()
            .push(association);
    }
    Ok(runs
        .into_iter()
        .filter_map(|run| {
            let associations = by_run.remove(&run.id)?;
            let deliverables = associations
                .into_iter()
                .filter_map(|association| {
                    model_map
                        .get(&association.deliverable_id)
                        .cloned()
                        .map(|model| to_info(model, Some(&association), Some(&run)))
                        .filter(is_user_visible_deliverable)
                })
                .collect::<Vec<_>>();
            (!deliverables.is_empty()).then_some(ConversationTurnDeliverableSet {
                turn_run_id: run.id,
                conversation_id,
                client_message_id: run.client_message_id,
                user_turn_id: None,
                started_at: run.started_at,
                completed_at: run.completed_at,
                deliverables,
            })
        })
        .collect())
}

pub async fn list_sets_for_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<ConversationTurnDeliverableSet>, DbError> {
    let runs = conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .order_by_asc(conversation_turn_run::Column::StartedAt)
        .all(conn)
        .await?;
    build_sets_for_runs(conn, conversation_id, runs).await
}

/// Return only deliverable sets that belong to the currently loaded transcript
/// page, plus an active run whose parser turn has not landed yet. This keeps a
/// long conversation detail read from statting and serializing its complete
/// deliverable ledger on every switch or history-page fetch.
pub async fn list_sets_for_turns(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turns: &[MessageTurn],
) -> Result<Vec<ConversationTurnDeliverableSet>, DbError> {
    let runs = conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .order_by_asc(conversation_turn_run::Column::StartedAt)
        .all(conn)
        .await?;
    let mut placeholders = runs
        .iter()
        .map(|run| ConversationTurnDeliverableSet {
            turn_run_id: run.id.clone(),
            conversation_id,
            client_message_id: run.client_message_id.clone(),
            user_turn_id: None,
            started_at: run.started_at,
            completed_at: run.completed_at,
            deliverables: Vec::new(),
        })
        .collect::<Vec<_>>();
    associate_sets_with_run_models(&mut placeholders, &runs, turns);
    let user_turn_by_run = placeholders
        .into_iter()
        .filter_map(|set| Some((set.turn_run_id, set.user_turn_id?)))
        .collect::<HashMap<_, _>>();
    let selected_runs = runs
        .into_iter()
        .filter(|run| {
            user_turn_by_run.contains_key(&run.id)
                || run.status == ConversationTurnRunStatus::Running
        })
        .collect::<Vec<_>>();
    let mut sets = build_sets_for_runs(conn, conversation_id, selected_runs).await?;
    for set in &mut sets {
        set.user_turn_id = user_turn_by_run.get(&set.turn_run_id).cloned();
    }
    Ok(sets)
}

fn associate_sets_with_run_models(
    sets: &mut [ConversationTurnDeliverableSet],
    runs: &[conversation_turn_run::Model],
    turns: &[MessageTurn],
) {
    if sets.is_empty() || turns.is_empty() {
        return;
    }
    let fingerprints = runs
        .iter()
        .filter_map(|run| Some((run.id.clone(), run.prompt_fingerprint.clone()?)))
        .collect::<HashMap<_, _>>();
    let candidates = turns
        .iter()
        .filter_map(|turn| Some((turn, crate::artifact_tracker::user_turn_fingerprint(turn)?)))
        .collect::<Vec<_>>();
    let mut used = HashSet::new();

    // A live detail may already have had its parser id patched to the exact
    // client id. Claim those first before fingerprint matching duplicate text.
    for set in sets.iter_mut() {
        let Some(client_id) = set.client_message_id.as_deref() else {
            continue;
        };
        if turns
            .iter()
            .any(|turn| turn.id == client_id && matches!(turn.role, crate::models::TurnRole::User))
        {
            set.user_turn_id = Some(client_id.to_string());
            used.insert(client_id.to_string());
        }
    }

    for set in sets.iter_mut().filter(|set| set.user_turn_id.is_none()) {
        let Some(fingerprint) = fingerprints.get(&set.turn_run_id) else {
            continue;
        };
        let best = candidates
            .iter()
            .filter(|(turn, candidate)| candidate == fingerprint && !used.contains(&turn.id))
            .min_by_key(|(turn, _)| {
                turn.timestamp
                    .timestamp_millis()
                    .abs_diff(set.started_at.timestamp_millis())
            });
        if let Some((turn, _)) = best {
            set.user_turn_id = Some(turn.id.clone());
            used.insert(turn.id.clone());
        }
    }

    // Rows written before prompt fingerprints were persisted can still be
    // placed safely when their run began near one unclaimed user turn in the
    // currently loaded history page. Never return a distant guess: an
    // unmatched set belongs in the history panel, not under the latest reply.
    for set in sets.iter_mut().filter(|set| set.user_turn_id.is_none()) {
        let best = turns
            .iter()
            .filter(|turn| matches!(turn.role, crate::models::TurnRole::User))
            .filter(|turn| !used.contains(&turn.id))
            .filter_map(|turn| {
                let distance = turn
                    .timestamp
                    .timestamp_millis()
                    .abs_diff(set.started_at.timestamp_millis());
                (distance <= 90_000).then_some((turn, distance))
            })
            .min_by_key(|(_, distance)| *distance);
        if let Some((turn, _)) = best {
            set.user_turn_id = Some(turn.id.clone());
            used.insert(turn.id.clone());
        }
    }
}

/// Bind deliverable runs to parser-stable user-turn ids before a detail is sent
/// to any client. The sender-only optimistic id remains useful during the live
/// turn, but a prompt fingerprint is the authoritative cross-device bridge
/// after reload. Matching is one-to-one; repeated identical prompts are
/// disambiguated by the nearest run start time.
pub async fn associate_sets_with_user_turns(
    conn: &DatabaseConnection,
    sets: &mut [ConversationTurnDeliverableSet],
    turns: &[MessageTurn],
) -> Result<(), DbError> {
    if sets.is_empty() || turns.is_empty() {
        return Ok(());
    }
    let run_ids = sets
        .iter()
        .map(|set| set.turn_run_id.clone())
        .collect::<Vec<_>>();
    let mut runs = Vec::new();
    for run_id_chunk in run_ids.chunks(500) {
        runs.extend(
            conversation_turn_run::Entity::find()
                .filter(conversation_turn_run::Column::Id.is_in(run_id_chunk.to_vec()))
                .all(conn)
                .await?,
        );
    }
    associate_sets_with_run_models(sets, &runs, turns);
    Ok(())
}

pub async fn hide_for_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
    ids: &[String],
) -> Result<u64, DbError> {
    if ids.is_empty() {
        return Ok(0);
    }
    if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return Err(DbError::Validation(
            "duplicate deliverable ids are not allowed".into(),
        ));
    }
    let txn = conn.begin().await?;
    let owned = conversation_deliverable::Entity::find()
        .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
        .filter(conversation_deliverable::Column::IsHidden.eq(false))
        .filter(conversation_deliverable::Column::Id.is_in(ids.to_vec()))
        .all(&txn)
        .await?;
    if owned.len() != ids.len() {
        return Err(DbError::NotFound(
            "one or more deliverables do not belong to this conversation".into(),
        ));
    }
    let result = conversation_deliverable::Entity::update_many()
        .col_expr(
            conversation_deliverable::Column::IsHidden,
            sea_orm::sea_query::Expr::value(true),
        )
        .col_expr(
            conversation_deliverable::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(Utc::now()),
        )
        .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
        .filter(conversation_deliverable::Column::IsHidden.eq(false))
        .filter(conversation_deliverable::Column::Id.is_in(ids.to_vec()))
        .exec(&txn)
        .await?;
    txn.commit().await?;
    Ok(result.rows_affected)
}

/// Resolve only database-owned ids scoped to one conversation. Callers never
/// provide a source path. Every item is revalidated immediately before use.
pub async fn resolve_for_access(
    conn: &DatabaseConnection,
    conversation_id: i32,
    ids: &[String],
) -> Result<Vec<ResolvedDeliverable>, DbError> {
    if ids.is_empty() {
        return Err(DbError::Validation(
            "at least one deliverable id is required".into(),
        ));
    }
    let unique = ids.iter().cloned().collect::<HashSet<_>>();
    if unique.len() != ids.len() {
        return Err(DbError::Validation(
            "duplicate deliverable ids are not allowed".into(),
        ));
    }
    let Some(conversation) = conversation::Entity::find_by_id(conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?
    else {
        return Err(DbError::NotFound(format!("conversation {conversation_id}")));
    };
    let mut allowed_roots = HashSet::new();
    if let Some(folder) = folder::Entity::find_by_id(conversation.folder_id)
        .filter(folder::Column::DeletedAt.is_null())
        .one(conn)
        .await?
    {
        if let Ok(path) = std::fs::canonicalize(folder.path) {
            allowed_roots.insert(path);
        }
    }
    for run in conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .all(conn)
        .await?
    {
        if let Ok(path) = std::fs::canonicalize(run.root_path) {
            allowed_roots.insert(path);
        }
    }
    let rows = conversation_deliverable::Entity::find()
        .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
        .filter(conversation_deliverable::Column::IsHidden.eq(false))
        .filter(conversation_deliverable::Column::Id.is_in(ids.to_vec()))
        .all(conn)
        .await?;
    if rows.len() != ids.len() {
        return Err(DbError::NotFound(
            "one or more deliverables do not belong to this conversation".into(),
        ));
    }
    let by_id = rows
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect::<HashMap<_, _>>();
    let mut resolved = Vec::with_capacity(ids.len());
    for id in ids {
        let model = by_id
            .get(id)
            .cloned()
            .ok_or_else(|| DbError::NotFound(format!("deliverable {id}")))?;
        let canonical_root = std::fs::canonicalize(&model.root_path)?;
        if !allowed_roots.contains(&canonical_root) {
            return Err(DbError::Validation(
                "deliverable root is not owned by this conversation".into(),
            ));
        }
        let inspected = inspect_persisted_path(&model.root_path, &model.path, &model.kind)?;
        resolved.push(ResolvedDeliverable {
            model,
            absolute_path: inspected.absolute_path,
        });
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::types::PromptInputBlock;
    use crate::db::service::artifact_service::{
        self, NewTurnRun, PendingFileChange, ReportedFileChange,
    };
    use crate::models::{AgentType, ContentBlock, MessageTurn, TurnRole};

    async fn seed_run(
        db: &crate::db::AppDatabase,
        conversation_id: i32,
        folder_id: i32,
        root: &Path,
        run_id: &str,
        input_paths: &[&str],
    ) {
        artifact_service::create_run(
            &db.conn,
            NewTurnRun {
                id: run_id.into(),
                conversation_id,
                connection_id: format!("conn-{run_id}"),
                client_message_id: Some(format!("message-{run_id}")),
                prompt_fingerprint: None,
                folder_id: Some(folder_id),
                root_path: root.to_string_lossy().to_string(),
                capture_incomplete: false,
                input_paths_json: serde_json::to_string(input_paths).unwrap(),
                expectation_json:
                    r#"{"publish_required":true,"expects_code_changes":true,"requested_paths":[]}"#
                        .into(),
            },
        )
        .await
        .expect("run");
    }

    #[tokio::test]
    async fn prompt_fingerprint_links_deliverables_to_a_durable_user_turn() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        let text = "生成 output/report.pdf";
        let fingerprint = crate::artifact_tracker::prompt_fingerprint(&[PromptInputBlock::Text {
            text: text.into(),
        }]);
        let run = artifact_service::create_run(
            &db.conn,
            NewTurnRun {
                id: "run-linked".into(),
                conversation_id,
                connection_id: "conn-linked".into(),
                client_message_id: Some("optimistic-only-on-sender".into()),
                prompt_fingerprint: fingerprint,
                folder_id: Some(folder_id),
                root_path: workspace.path().to_string_lossy().to_string(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
                expectation_json: r#"{"publish_required":true,"expects_code_changes":false,"requested_paths":["output/report.pdf"]}"#.into(),
            },
        )
        .await
        .unwrap();
        let mut sets = vec![ConversationTurnDeliverableSet {
            turn_run_id: run.id,
            conversation_id,
            client_message_id: run.client_message_id,
            user_turn_id: None,
            // Deliberately far from the parser timestamp: this association is
            // content-based and therefore stable across clients/clocks.
            started_at: run.started_at,
            completed_at: None,
            deliverables: Vec::new(),
        }];
        let turns = vec![MessageTurn {
            id: "parser-stable-user-id".into(),
            role: TurnRole::User,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            timestamp: run.started_at + chrono::Duration::hours(8),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
        }];

        associate_sets_with_user_turns(&db.conn, &mut sets, &turns)
            .await
            .unwrap();
        assert_eq!(
            sets[0].user_turn_id.as_deref(),
            Some("parser-stable-user-id")
        );
    }

    #[tokio::test]
    async fn timestamp_fallback_never_binds_a_deliverable_run_to_an_assistant_turn() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        let run = artifact_service::create_run(
            &db.conn,
            NewTurnRun {
                id: "run-timestamp-user-only".into(),
                conversation_id,
                connection_id: "conn-timestamp-user-only".into(),
                client_message_id: None,
                prompt_fingerprint: None,
                folder_id: Some(folder_id),
                root_path: workspace.path().to_string_lossy().to_string(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
                expectation_json: "{}".into(),
            },
        )
        .await
        .unwrap();
        let mut sets = vec![ConversationTurnDeliverableSet {
            turn_run_id: run.id.clone(),
            conversation_id,
            client_message_id: None,
            user_turn_id: None,
            started_at: run.started_at,
            completed_at: None,
            deliverables: Vec::new(),
        }];
        let turns = vec![
            MessageTurn {
                id: "assistant-nearest".into(),
                role: TurnRole::Assistant,
                blocks: Vec::new(),
                timestamp: run.started_at,
                usage: None,
                duration_ms: None,
                model: None,
                completed_at: None,
            },
            MessageTurn {
                id: "user-nearby".into(),
                role: TurnRole::User,
                blocks: vec![ContentBlock::Text {
                    text: "legacy prompt".into(),
                }],
                timestamp: run.started_at + chrono::Duration::seconds(1),
                usage: None,
                duration_ms: None,
                model: None,
                completed_at: None,
            },
        ];

        associate_sets_with_user_turns(&db.conn, &mut sets, &turns)
            .await
            .unwrap();
        assert_eq!(sets[0].user_turn_id.as_deref(), Some("user-nearby"));
    }

    fn verified(root: &Path, path: &str, title: &str) -> VerifiedDeliverable {
        let absolute = root.join(path);
        let metadata = std::fs::metadata(&absolute).unwrap();
        VerifiedDeliverable {
            root_path: std::fs::canonicalize(root)
                .unwrap()
                .to_string_lossy()
                .to_string(),
            path: path.into(),
            kind: "file".into(),
            title: title.into(),
            description: None,
            role: "primary".into(),
            category: "standalone_output".into(),
            change_kind: "created".into(),
            file_name: absolute.file_name().unwrap().to_string_lossy().to_string(),
            extension: clean_extension(&absolute),
            size_bytes: i64::try_from(metadata.len()).ok(),
            modified_at: metadata.modified().ok().map(DateTime::<Utc>::from),
            is_valid: true,
            invalid_reason: None,
        }
    }

    #[test]
    fn inference_filters_internal_outputs_without_restricting_explicit_declarations() {
        assert!(inference_path_allowed("src/widget.test.ts"));
        assert!(!inference_path_allowed("Cargo.lock"));
        assert!(inference_path_allowed(".env.example"));
        assert!(!inference_path_allowed(".eslintcache"));
        assert!(!inference_path_allowed("target/debug/app"));
        assert!(!inference_path_allowed("draft.html"));
        assert!(!inference_path_allowed("report.test.pdf"));
        assert!(!inference_path_allowed("qa_page_1.png"));
        assert!(!inference_path_allowed("qa/page_1.png"));
        assert!(!inference_path_allowed("_qa/page-01.png"));
        assert!(!inference_path_allowed("_rendered/page-002.png"));
        assert!(!inference_path_allowed("_backups/final.pdf"));
        assert!(!inference_path_allowed("report_backup.pdf"));
        assert!(!inference_path_allowed("report-old.pdf"));
        assert!(!inference_path_allowed("_probe.pdf"));
        assert!(!inference_path_allowed("build_report.py"));
        assert!(!inference_path_allowed("STATUS.md"));
        assert!(!inference_path_allowed("render_cache/page_1.png"));
        assert!(!inference_path_allowed("visual_check_page_1.png"));
    }

    #[tokio::test]
    async fn second_declaration_replaces_only_the_same_turn_and_keeps_history() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("report.pdf"), b"one").unwrap();
        std::fs::write(workspace.path().join("appendix.pdf"), b"two").unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_run(
            &db,
            conversation_id,
            folder_id,
            workspace.path(),
            "run-1",
            &[],
        )
        .await;

        let first = replace_declared_for_turn(
            &db.conn,
            conversation_id,
            "run-1",
            vec![
                verified(workspace.path(), "report.pdf", "Report"),
                verified(workspace.path(), "appendix.pdf", "Appendix"),
            ],
        )
        .await
        .unwrap();
        let retained_id = first[0].id.clone();
        replace_declared_for_turn(
            &db.conn,
            conversation_id,
            "run-1",
            vec![verified(workspace.path(), "report.pdf", "Final report")],
        )
        .await
        .unwrap();

        let turn = list_for_turn(&db.conn, conversation_id, "run-1")
            .await
            .unwrap();
        assert_eq!(turn.len(), 1);
        assert_eq!(turn[0].id, retained_id);
        assert_eq!(turn[0].title, "Final report");
        assert_eq!(turn[0].source, SOURCE_DECLARED);
        assert_eq!(
            list_for_conversation(&db.conn, conversation_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn merge_appends_and_replace_resets_the_same_turn_set() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("report.pdf"), b"one").unwrap();
        std::fs::write(workspace.path().join("appendix.docx"), b"two").unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_run(
            &db,
            conversation_id,
            folder_id,
            workspace.path(),
            "run-modes",
            &[],
        )
        .await;

        save_declared_for_turn(
            &db.conn,
            conversation_id,
            "run-modes",
            vec![verified(workspace.path(), "report.pdf", "Report")],
            false,
        )
        .await
        .unwrap();
        let mut supporting = verified(workspace.path(), "appendix.docx", "Appendix");
        supporting.role = "supporting".into();
        save_declared_for_turn(
            &db.conn,
            conversation_id,
            "run-modes",
            vec![supporting],
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            list_for_turn(&db.conn, conversation_id, "run-modes")
                .await
                .unwrap()
                .len(),
            2
        );

        replace_declared_for_turn(
            &db.conn,
            conversation_id,
            "run-modes",
            vec![verified(workspace.path(), "report.pdf", "Final report")],
        )
        .await
        .unwrap();
        let replaced = list_for_turn(&db.conn, conversation_id, "run-modes")
            .await
            .unwrap();
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].title, "Final report");
    }

    #[tokio::test]
    async fn explicit_script_is_visible_even_though_inference_would_filter_it() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("install.py"), b"print('ok')").unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_run(
            &db,
            conversation_id,
            folder_id,
            workspace.path(),
            "run-declared-script",
            &[],
        )
        .await;

        replace_declared_for_turn(
            &db.conn,
            conversation_id,
            "run-declared-script",
            vec![verified(workspace.path(), "install.py", "Installer")],
        )
        .await
        .unwrap();

        let listed = list_for_turn(&db.conn, conversation_id, "run-declared-script")
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].source, SOURCE_DECLARED);
        assert_eq!(listed[0].path, "install.py");
    }

    #[tokio::test]
    async fn declared_set_suppresses_tracker_noise_and_repairs_legacy_mixed_rows_on_read() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        for (path, contents) in [
            ("final.pdf", b"pdf".as_slice()),
            ("merged.docx", b"docx".as_slice()),
            ("build_charging_documents.py", b"print('build')".as_slice()),
            ("qa_page_1.png", b"png-1".as_slice()),
            ("qa_page_2.png", b"png-2".as_slice()),
        ] {
            std::fs::write(workspace.path().join(path), contents).unwrap();
        }
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_run(
            &db,
            conversation_id,
            folder_id,
            workspace.path(),
            "run-declared-authoritative",
            &[],
        )
        .await;

        let mut supporting = verified(workspace.path(), "merged.docx", "Merged DOCX");
        supporting.role = "supporting".into();
        replace_declared_for_turn(
            &db.conn,
            conversation_id,
            "run-declared-authoritative",
            vec![
                verified(workspace.path(), "final.pdf", "Final PDF"),
                supporting,
            ],
        )
        .await
        .unwrap();
        assert!(mark_declaration_result(
            &db.conn,
            conversation_id,
            "run-declared-authoritative",
            "success",
            None,
        )
        .await
        .unwrap());

        artifact_service::upsert_changes(
            &db.conn,
            "run-declared-authoritative",
            [
                "build_charging_documents.py",
                "qa_page_1.png",
                "qa_page_2.png",
            ]
            .into_iter()
            .map(|path| PendingFileChange {
                path: path.into(),
                kind: ConversationTurnFileChangeKind::Created,
                attribution: "exclusive".into(),
            })
            .collect(),
        )
        .await
        .unwrap();
        let final_metadata = std::fs::metadata(workspace.path().join("final.pdf")).unwrap();
        artifact_service::upsert_reported_changes(
            &db.conn,
            "run-declared-authoritative",
            vec![ReportedFileChange {
                path: "final.pdf".into(),
                kind: ConversationTurnFileChangeKind::Modified,
                final_exists: true,
                size_bytes: i64::try_from(final_metadata.len()).ok(),
                modified_at: final_metadata.modified().ok().map(DateTime::<Utc>::from),
            }],
        )
        .await
        .unwrap();
        for change in artifact_service::list_changes_for_run(&db.conn, "run-declared-authoritative")
            .await
            .unwrap()
        {
            let metadata = std::fs::metadata(workspace.path().join(&change.path)).unwrap();
            artifact_service::update_final_state(
                &db.conn,
                change,
                true,
                i64::try_from(metadata.len()).ok(),
                metadata.modified().ok().map(DateTime::<Utc>::from),
            )
            .await
            .unwrap();
        }
        artifact_service::finish_run(
            &db.conn,
            "run-declared-authoritative",
            ConversationTurnRunStatus::Completed,
            None,
        )
        .await
        .unwrap();

        assert!(
            infer_for_turn(&db.conn, conversation_id, "run-declared-authoritative")
                .await
                .unwrap()
                .is_empty(),
            "successful publish must prevent tracker files from joining the set"
        );

        // Recreate the faulty shape written by older releases: inference was
        // appended after a successful replace declaration. The read side must
        // heal this without requiring a destructive data migration.
        let mut script = verified(
            workspace.path(),
            "build_charging_documents.py",
            "Build script",
        );
        script.role = "supporting".into();
        script.category = "code_change".into();
        save_turn_set(
            &db.conn,
            conversation_id,
            "run-declared-authoritative",
            SOURCE_INFERRED,
            vec![
                script,
                verified(workspace.path(), "qa_page_1.png", "QA page 1"),
                verified(workspace.path(), "qa_page_2.png", "QA page 2"),
            ],
            false,
            false,
        )
        .await
        .unwrap();

        let turn = list_for_turn(&db.conn, conversation_id, "run-declared-authoritative")
            .await
            .unwrap();
        assert_eq!(
            turn.iter()
                .map(|item| item.path.as_str())
                .collect::<Vec<_>>(),
            vec!["final.pdf", "merged.docx"]
        );
        assert!(turn.iter().all(|item| item.source == SOURCE_DECLARED));

        let sets = list_sets_for_conversation(&db.conn, conversation_id)
            .await
            .unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].deliverables.len(), 2);
        let conversation = list_for_conversation(&db.conn, conversation_id)
            .await
            .unwrap();
        assert_eq!(conversation.len(), 2);
        assert!(conversation
            .iter()
            .all(|item| !item.path.starts_with("qa_") && item.extension.as_deref() != Some("py")));
    }

    #[tokio::test]
    async fn legacy_inferred_qa_scripts_and_status_files_are_filtered_on_read() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join("_qa")).unwrap();
        for (path, contents) in [
            ("final.pdf", b"final".as_slice()),
            ("_probe.pdf", b"probe".as_slice()),
            ("_qa/page-01.png", b"preview".as_slice()),
            ("build_report.py", b"build".as_slice()),
            ("STATUS.md", b"working".as_slice()),
        ] {
            std::fs::write(workspace.path().join(path), contents).unwrap();
        }
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_run(
            &db,
            conversation_id,
            folder_id,
            workspace.path(),
            "run-legacy-noise",
            &[],
        )
        .await;
        artifact_service::finish_run(
            &db.conn,
            "run-legacy-noise",
            ConversationTurnRunStatus::Completed,
            None,
        )
        .await
        .unwrap();
        let mut script = verified(workspace.path(), "build_report.py", "Build script");
        script.category = "code_change".into();
        let mut status = verified(workspace.path(), "STATUS.md", "Status");
        status.category = "code_change".into();
        save_turn_set(
            &db.conn,
            conversation_id,
            "run-legacy-noise",
            SOURCE_INFERRED,
            vec![
                verified(workspace.path(), "final.pdf", "Final"),
                verified(workspace.path(), "_probe.pdf", "Probe"),
                verified(workspace.path(), "_qa/page-01.png", "QA page"),
                script,
                status,
            ],
            false,
            false,
        )
        .await
        .unwrap();

        let history = list_history_page(&db.conn, conversation_id, 0, 25)
            .await
            .unwrap();
        assert_eq!(history.total, 1);
        assert_eq!(history.items[0].latest.path, "final.pdf");
        let sets = list_sets_for_conversation(&db.conn, conversation_id)
            .await
            .unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].deliverables.len(), 1);
        assert_eq!(sets[0].deliverables[0].path, "final.pdf");
    }

    #[tokio::test]
    async fn same_path_in_later_turn_is_deduplicated_but_both_turns_keep_history() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("report.pdf"), b"version one").unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_run(
            &db,
            conversation_id,
            folder_id,
            workspace.path(),
            "run-1",
            &[],
        )
        .await;
        let first = replace_declared_for_turn(
            &db.conn,
            conversation_id,
            "run-1",
            vec![verified(workspace.path(), "report.pdf", "First version")],
        )
        .await
        .unwrap();
        artifact_service::finish_run(
            &db.conn,
            "run-1",
            ConversationTurnRunStatus::Completed,
            None,
        )
        .await
        .unwrap();

        std::fs::write(workspace.path().join("report.pdf"), b"version two").unwrap();
        seed_run(
            &db,
            conversation_id,
            folder_id,
            workspace.path(),
            "run-2",
            &[],
        )
        .await;
        let second = replace_declared_for_turn(
            &db.conn,
            conversation_id,
            "run-2",
            vec![verified(workspace.path(), "report.pdf", "Final version")],
        )
        .await
        .unwrap();

        assert_eq!(first[0].id, second[0].id);
        let aggregate = list_for_conversation(&db.conn, conversation_id)
            .await
            .unwrap();
        assert_eq!(aggregate.len(), 1);
        assert_eq!(aggregate[0].title, "Final version");
        assert_eq!(aggregate[0].turn_run_id.as_deref(), Some("run-2"));

        let history = list_history_page(&db.conn, conversation_id, 0, 25)
            .await
            .unwrap();
        assert_eq!(history.total, 1);
        assert!(!history.has_more);
        assert_eq!(history.items[0].latest.title, "Final version");
        assert_eq!(history.items[0].versions.len(), 2);
        assert_eq!(history.items[0].versions[0].title, "Final version");
        assert_eq!(history.items[0].versions[1].title, "First version");

        let sets = list_sets_for_conversation(&db.conn, conversation_id)
            .await
            .unwrap();
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0].deliverables[0].title, "First version");
        assert_eq!(sets[1].deliverables[0].title, "Final version");
    }

    #[tokio::test]
    async fn transcript_page_only_loads_deliverables_for_visible_turns() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("old.pdf"), b"old").unwrap();
        std::fs::write(workspace.path().join("visible.pdf"), b"visible").unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        for (run_id, path) in [("run-old", "old.pdf"), ("run-visible", "visible.pdf")] {
            seed_run(
                &db,
                conversation_id,
                folder_id,
                workspace.path(),
                run_id,
                &[],
            )
            .await;
            replace_declared_for_turn(
                &db.conn,
                conversation_id,
                run_id,
                vec![verified(workspace.path(), path, path)],
            )
            .await
            .unwrap();
            artifact_service::finish_run(
                &db.conn,
                run_id,
                ConversationTurnRunStatus::Completed,
                None,
            )
            .await
            .unwrap();
        }
        let visible_run = conversation_turn_run::Entity::find_by_id("run-visible")
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        let turns = vec![MessageTurn {
            id: "message-run-visible".into(),
            role: TurnRole::User,
            blocks: vec![ContentBlock::Text {
                text: "visible".into(),
            }],
            timestamp: visible_run.started_at,
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
        }];

        let sets = list_sets_for_turns(&db.conn, conversation_id, &turns)
            .await
            .unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].turn_run_id, "run-visible");
        assert_eq!(sets[0].user_turn_id.as_deref(), Some("message-run-visible"));
        assert_eq!(sets[0].deliverables[0].path, "visible.pdf");
    }

    #[tokio::test]
    async fn explicit_empty_declaration_still_runs_fallback_settlement() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("generated.pdf"), b"pdf").unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_run(
            &db,
            conversation_id,
            folder_id,
            workspace.path(),
            "run-empty",
            &[],
        )
        .await;
        replace_declared_for_turn(&db.conn, conversation_id, "run-empty", Vec::new())
            .await
            .unwrap();
        artifact_service::upsert_changes(
            &db.conn,
            "run-empty",
            vec![PendingFileChange {
                path: "generated.pdf".into(),
                kind: ConversationTurnFileChangeKind::Created,
                attribution: "exclusive".into(),
            }],
        )
        .await
        .unwrap();
        let change = artifact_service::list_changes_for_run(&db.conn, "run-empty")
            .await
            .unwrap()
            .pop()
            .unwrap();
        artifact_service::update_final_state(
            &db.conn,
            change,
            true,
            Some(3),
            std::fs::metadata(workspace.path().join("generated.pdf"))
                .unwrap()
                .modified()
                .ok()
                .map(DateTime::<Utc>::from),
        )
        .await
        .unwrap();
        artifact_service::finish_run(
            &db.conn,
            "run-empty",
            ConversationTurnRunStatus::Completed,
            None,
        )
        .await
        .unwrap();

        let inferred = infer_for_turn(&db.conn, conversation_id, "run-empty")
            .await
            .unwrap();
        assert_eq!(inferred.len(), 1);
        assert_eq!(inferred[0].path, "generated.pdf");
        assert_eq!(
            list_for_conversation(&db.conn, conversation_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn inference_keeps_final_outputs_and_leaves_code_changes_in_the_artifact_tracker() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join("src")).unwrap();
        std::fs::create_dir_all(workspace.path().join("_qa")).unwrap();
        std::fs::write(workspace.path().join("input.pdf"), b"input").unwrap();
        std::fs::write(workspace.path().join("draft.html"), b"draft").unwrap();
        std::fs::write(workspace.path().join("report.test.pdf"), b"test").unwrap();
        std::fs::write(workspace.path().join("final.pdf"), b"final").unwrap();
        std::fs::write(workspace.path().join("src/widget.test.ts"), b"test code").unwrap();
        std::fs::write(workspace.path().join("_qa/page-01.png"), b"preview").unwrap();
        std::fs::write(workspace.path().join("build_report.py"), b"build").unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_run(
            &db,
            conversation_id,
            folder_id,
            workspace.path(),
            "run-infer",
            &["input.pdf"],
        )
        .await;
        artifact_service::upsert_changes(
            &db.conn,
            "run-infer",
            [
                ("input.pdf", ConversationTurnFileChangeKind::Created),
                ("draft.html", ConversationTurnFileChangeKind::Created),
                ("report.test.pdf", ConversationTurnFileChangeKind::Created),
                ("final.pdf", ConversationTurnFileChangeKind::Created),
                (
                    "src/widget.test.ts",
                    ConversationTurnFileChangeKind::Modified,
                ),
                ("_qa/page-01.png", ConversationTurnFileChangeKind::Created),
                ("build_report.py", ConversationTurnFileChangeKind::Created),
            ]
            .into_iter()
            .map(|(path, kind)| PendingFileChange {
                path: path.into(),
                kind,
                attribution: "exclusive".into(),
            })
            .collect(),
        )
        .await
        .unwrap();
        for change in artifact_service::list_changes_for_run(&db.conn, "run-infer")
            .await
            .unwrap()
        {
            let path = workspace.path().join(&change.path);
            let metadata = std::fs::metadata(path).unwrap();
            artifact_service::update_final_state(
                &db.conn,
                change,
                true,
                i64::try_from(metadata.len()).ok(),
                metadata.modified().ok().map(DateTime::<Utc>::from),
            )
            .await
            .unwrap();
        }
        artifact_service::finish_run(
            &db.conn,
            "run-infer",
            ConversationTurnRunStatus::Completed,
            None,
        )
        .await
        .unwrap();

        let inferred = infer_for_turn(&db.conn, conversation_id, "run-infer")
            .await
            .unwrap();
        assert_eq!(inferred.len(), 1);
        let by_path = inferred
            .into_iter()
            .map(|item| (item.path.clone(), item))
            .collect::<HashMap<_, _>>();
        assert_eq!(by_path["final.pdf"].source, SOURCE_INFERRED);
        assert_eq!(by_path["final.pdf"].category, "standalone_output");
        assert_eq!(by_path["final.pdf"].role, "primary");
        assert!(!by_path.contains_key("src/widget.test.ts"));

        // Old supporting roles remain supporting on read. The client renders
        // every filtered inference-only output, so role recovery must not
        // manufacture extra primary files.
        let association = conversation_turn_deliverable::Entity::find()
            .filter(conversation_turn_deliverable::Column::TurnRunId.eq("run-infer"))
            .filter(
                conversation_turn_deliverable::Column::DeliverableId
                    .eq(by_path["final.pdf"].id.clone()),
            )
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        let mut legacy = association.into_active_model();
        legacy.role = Set("supporting".into());
        legacy.update(&db.conn).await.unwrap();

        let sets = list_sets_for_conversation(&db.conn, conversation_id)
            .await
            .unwrap();
        let legacy_pdf = sets
            .iter()
            .find(|set| set.turn_run_id == "run-infer")
            .and_then(|set| {
                set.deliverables
                    .iter()
                    .find(|item| item.path == "final.pdf")
            })
            .unwrap();
        assert_eq!(legacy_pdf.role, "supporting");
    }

    #[tokio::test]
    async fn missing_prompt_path_marks_terminal_settlement_incomplete() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        artifact_service::create_run(
            &db.conn,
            NewTurnRun {
                id: "run-missing".into(),
                conversation_id,
                connection_id: "conn-missing".into(),
                client_message_id: Some("message-missing".into()),
                prompt_fingerprint: None,
                folder_id: Some(folder_id),
                root_path: workspace.path().to_string_lossy().to_string(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
                expectation_json: r#"{"publish_required":true,"expects_code_changes":false,"requested_paths":["out/report.pdf"]}"#.into(),
            },
        )
        .await
        .unwrap();
        artifact_service::finish_run(
            &db.conn,
            "run-missing",
            ConversationTurnRunStatus::Completed,
            None,
        )
        .await
        .unwrap();

        assert!(infer_for_turn(&db.conn, conversation_id, "run-missing")
            .await
            .unwrap()
            .is_empty());
        let run = conversation_turn_run::Entity::find_by_id("run-missing")
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.settlement_status, "settled_incomplete");
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&run.missing_expected_paths_json).unwrap(),
            vec!["out/report.pdf"]
        );
    }

    #[tokio::test]
    async fn expected_source_path_is_recorded_but_not_user_visible_until_declared() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join("src")).unwrap();
        std::fs::write(workspace.path().join("src/lib.rs"), b"pub fn changed() {}").unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        artifact_service::create_run(
            &db.conn,
            NewTurnRun {
                id: "run-ambiguous-expected".into(),
                conversation_id,
                connection_id: "conn-ambiguous-expected".into(),
                client_message_id: Some("message-ambiguous-expected".into()),
                prompt_fingerprint: None,
                folder_id: Some(folder_id),
                root_path: workspace.path().to_string_lossy().to_string(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
                expectation_json: r#"{"publish_required":true,"expects_code_changes":true,"requested_paths":["src/lib.rs"]}"#.into(),
            },
        )
        .await
        .unwrap();
        artifact_service::upsert_changes(
            &db.conn,
            "run-ambiguous-expected",
            vec![PendingFileChange {
                path: "src/lib.rs".into(),
                kind: ConversationTurnFileChangeKind::Modified,
                attribution: "ambiguous".into(),
            }],
        )
        .await
        .unwrap();
        let change = artifact_service::list_changes_for_run(&db.conn, "run-ambiguous-expected")
            .await
            .unwrap()
            .pop()
            .unwrap();
        let metadata = std::fs::metadata(workspace.path().join("src/lib.rs")).unwrap();
        artifact_service::update_final_state(
            &db.conn,
            change,
            true,
            i64::try_from(metadata.len()).ok(),
            metadata.modified().ok().map(DateTime::<Utc>::from),
        )
        .await
        .unwrap();
        artifact_service::finish_run(
            &db.conn,
            "run-ambiguous-expected",
            ConversationTurnRunStatus::Completed,
            None,
        )
        .await
        .unwrap();

        let settled = infer_for_turn(&db.conn, conversation_id, "run-ambiguous-expected")
            .await
            .unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].category, "code_change");
        assert_eq!(settled[0].role, "supporting");
        assert!(
            list_for_turn(&db.conn, conversation_id, "run-ambiguous-expected")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn one_unambiguous_standalone_fallback_may_be_primary() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("report.pdf"), b"report").unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_run(
            &db,
            conversation_id,
            folder_id,
            workspace.path(),
            "run-ambiguous-output",
            &[],
        )
        .await;
        artifact_service::upsert_changes(
            &db.conn,
            "run-ambiguous-output",
            vec![PendingFileChange {
                path: "report.pdf".into(),
                kind: ConversationTurnFileChangeKind::Created,
                attribution: "ambiguous".into(),
            }],
        )
        .await
        .unwrap();
        let change = artifact_service::list_changes_for_run(&db.conn, "run-ambiguous-output")
            .await
            .unwrap()
            .pop()
            .unwrap();
        let metadata = std::fs::metadata(workspace.path().join("report.pdf")).unwrap();
        artifact_service::update_final_state(
            &db.conn,
            change,
            true,
            i64::try_from(metadata.len()).ok(),
            metadata.modified().ok().map(DateTime::<Utc>::from),
        )
        .await
        .unwrap();
        artifact_service::finish_run(
            &db.conn,
            "run-ambiguous-output",
            ConversationTurnRunStatus::Completed,
            None,
        )
        .await
        .unwrap();

        let settled = infer_for_turn(&db.conn, conversation_id, "run-ambiguous-output")
            .await
            .unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].category, "standalone_output");
        assert_eq!(settled[0].role, "primary");
    }

    #[tokio::test]
    async fn deleted_file_becomes_invalid_and_cannot_be_resolved() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("报告 (最终).docx"), b"docx").unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_run(
            &db,
            conversation_id,
            folder_id,
            workspace.path(),
            "run-delete",
            &[],
        )
        .await;
        let saved = replace_declared_for_turn(
            &db.conn,
            conversation_id,
            "run-delete",
            vec![verified(workspace.path(), "报告 (最终).docx", "报告")],
        )
        .await
        .unwrap();
        std::fs::remove_file(workspace.path().join("报告 (最终).docx")).unwrap();

        let listed = list_for_conversation(&db.conn, conversation_id)
            .await
            .unwrap();
        assert!(listed.is_empty(), "invalid records are hidden by default");
        let row = conversation_deliverable::Entity::find_by_id(saved[0].id.clone())
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert!(!row.is_valid);
        assert_eq!(row.invalid_reason.as_deref(), Some("file_not_found"));
        assert!(
            resolve_for_access(&db.conn, conversation_id, &[saved[0].id.clone()])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn deliverable_ids_are_scoped_to_their_conversation() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let first_workspace = tempfile::tempdir().unwrap();
        let second_workspace = tempfile::tempdir().unwrap();
        std::fs::write(first_workspace.path().join("private.pdf"), b"private").unwrap();
        let first_folder =
            crate::db::test_helpers::seed_folder(&db, &first_workspace.path().to_string_lossy())
                .await;
        let second_folder =
            crate::db::test_helpers::seed_folder(&db, &second_workspace.path().to_string_lossy())
                .await;
        let first_conversation =
            crate::db::test_helpers::seed_conversation(&db, first_folder, AgentType::Codex).await;
        let second_conversation =
            crate::db::test_helpers::seed_conversation(&db, second_folder, AgentType::Codex).await;
        seed_run(
            &db,
            first_conversation,
            first_folder,
            first_workspace.path(),
            "run-private",
            &[],
        )
        .await;
        let saved = replace_declared_for_turn(
            &db.conn,
            first_conversation,
            "run-private",
            vec![verified(first_workspace.path(), "private.pdf", "Private")],
        )
        .await
        .unwrap();

        let error = resolve_for_access(
            &db.conn,
            second_conversation,
            std::slice::from_ref(&saved[0].id),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn mixed_foreign_hide_is_rejected_without_hiding_owned_rows() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let first_workspace = tempfile::tempdir().unwrap();
        let second_workspace = tempfile::tempdir().unwrap();
        std::fs::write(first_workspace.path().join("first.pdf"), b"first").unwrap();
        std::fs::write(second_workspace.path().join("second.pdf"), b"second").unwrap();
        let first_folder =
            crate::db::test_helpers::seed_folder(&db, &first_workspace.path().to_string_lossy())
                .await;
        let second_folder =
            crate::db::test_helpers::seed_folder(&db, &second_workspace.path().to_string_lossy())
                .await;
        let first_conversation =
            crate::db::test_helpers::seed_conversation(&db, first_folder, AgentType::Codex).await;
        let second_conversation =
            crate::db::test_helpers::seed_conversation(&db, second_folder, AgentType::Codex).await;
        seed_run(
            &db,
            first_conversation,
            first_folder,
            first_workspace.path(),
            "run-first-hide",
            &[],
        )
        .await;
        seed_run(
            &db,
            second_conversation,
            second_folder,
            second_workspace.path(),
            "run-second-hide",
            &[],
        )
        .await;
        let first = replace_declared_for_turn(
            &db.conn,
            first_conversation,
            "run-first-hide",
            vec![verified(first_workspace.path(), "first.pdf", "First")],
        )
        .await
        .unwrap();
        let second = replace_declared_for_turn(
            &db.conn,
            second_conversation,
            "run-second-hide",
            vec![verified(second_workspace.path(), "second.pdf", "Second")],
        )
        .await
        .unwrap();

        let error = hide_for_conversation(
            &db.conn,
            first_conversation,
            &[first[0].id.clone(), second[0].id.clone()],
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DbError::NotFound(_)));
        let still_visible = list_for_conversation(&db.conn, first_conversation)
            .await
            .unwrap();
        assert_eq!(still_visible.len(), 1);
        assert_eq!(still_visible[0].id, first[0].id);
    }

    #[tokio::test]
    async fn deliverables_survive_database_close_and_server_style_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("restart.pdf"), b"persistent").unwrap();
        let db = crate::db::init_database(&data_dir, "deliverable-test")
            .await
            .unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_run(
            &db,
            conversation_id,
            folder_id,
            &workspace,
            "run-restart",
            &[],
        )
        .await;
        replace_declared_for_turn(
            &db.conn,
            conversation_id,
            "run-restart",
            vec![verified(&workspace, "restart.pdf", "Restart-safe report")],
        )
        .await
        .unwrap();
        db.conn.close().await.unwrap();

        let reopened = crate::db::init_database(&data_dir, "deliverable-test")
            .await
            .unwrap();
        let listed = list_for_conversation(&reopened.conn, conversation_id)
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "restart.pdf");
        assert_eq!(listed[0].title, "Restart-safe report");
        assert!(listed[0].is_valid);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn replacing_a_deliverable_with_an_escaping_symlink_is_rejected() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("report.pdf"), b"safe").unwrap();
        std::fs::write(outside.path().join("secret.pdf"), b"secret").unwrap();
        let folder_id =
            crate::db::test_helpers::seed_folder(&db, &workspace.path().to_string_lossy()).await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_run(
            &db,
            conversation_id,
            folder_id,
            workspace.path(),
            "run-symlink",
            &[],
        )
        .await;
        let saved = replace_declared_for_turn(
            &db.conn,
            conversation_id,
            "run-symlink",
            vec![verified(workspace.path(), "report.pdf", "Report")],
        )
        .await
        .unwrap();

        std::fs::remove_file(workspace.path().join("report.pdf")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.pdf"),
            workspace.path().join("report.pdf"),
        )
        .unwrap();

        let error = resolve_for_access(
            &db.conn,
            conversation_id,
            std::slice::from_ref(&saved[0].id),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DbError::Validation(_)));
        let listed = list_for_conversation(&db.conn, conversation_id)
            .await
            .unwrap();
        assert!(listed.is_empty(), "unsafe records are hidden by default");
        let row = conversation_deliverable::Entity::find_by_id(saved[0].id.clone())
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert!(!row.is_valid);
        assert_eq!(
            row.invalid_reason.as_deref(),
            Some("unsafe_or_changed_path")
        );
    }
}
