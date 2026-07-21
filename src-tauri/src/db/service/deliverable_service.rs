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
    conversation_turn_file_change, folder,
};
use crate::db::error::DbError;
use crate::models::{ConversationDeliverable, ConversationTurnDeliverableSet};

pub const SOURCE_DECLARED: &str = "declared";
pub const SOURCE_INFERRED: &str = "inferred";

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
    pub file_name: String,
    pub extension: Option<String>,
    pub size_bytes: Option<i64>,
    pub modified_at: Option<DateTime<Utc>>,
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

/// Record that publish_deliverables was attempted for this running turn. Even
/// an invalid declaration is authoritative evidence that fallback inference
/// must not guess a different set after the turn completes; the previously
/// accepted set (if any) remains untouched until a valid replacement arrives.
pub async fn mark_declaration_attempt(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turn_run_id: &str,
) -> Result<bool, DbError> {
    let Some(run) = conversation_turn_run::Entity::find_by_id(turn_run_id.to_string())
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .filter(conversation_turn_run::Column::Status.eq(ConversationTurnRunStatus::Running))
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    let mut active = run.into_active_model();
    active.deliverables_declared_at = Set(Some(Utc::now()));
    active.update(conn).await?;
    Ok(true)
}

async fn replace_turn_set(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turn_run_id: &str,
    source: &str,
    items: Vec<VerifiedDeliverable>,
    require_running: bool,
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
    if !require_running {
        if run.status != ConversationTurnRunStatus::Completed {
            return Err(DbError::Validation(
                "fallback inference is only allowed for completed turns".into(),
            ));
        }
        if run.capture_incomplete || run.deliverables_declared_at.is_some() {
            return Ok(Vec::new());
        }
    }

    let now = Utc::now();
    if source == SOURCE_DECLARED {
        let mut active_run = run.clone().into_active_model();
        active_run.deliverables_declared_at = Set(Some(now));
        active_run.update(&txn).await?;
    }

    let previous = conversation_turn_deliverable::Entity::find()
        .filter(conversation_turn_deliverable::Column::TurnRunId.eq(turn_run_id.to_string()))
        .all(&txn)
        .await?;
    let previous_ids = previous
        .iter()
        .map(|row| row.deliverable_id.clone())
        .collect::<HashSet<_>>();
    conversation_turn_deliverable::Entity::delete_many()
        .filter(conversation_turn_deliverable::Column::TurnRunId.eq(turn_run_id.to_string()))
        .exec(&txn)
        .await?;

    let mut saved_pairs = Vec::with_capacity(items.len());
    let mut retained_ids = HashSet::new();
    for (position, item) in items.into_iter().enumerate() {
        let position = i32::try_from(position).unwrap_or(i32::MAX);
        let existing = conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
            .filter(conversation_deliverable::Column::RootPath.eq(item.root_path.clone()))
            .filter(conversation_deliverable::Column::Path.eq(item.path.clone()))
            .one(&txn)
            .await?;

        let model = if let Some(existing) = existing {
            let mut active = existing.into_active_model();
            active.turn_run_id = Set(Some(turn_run_id.to_string()));
            active.kind = Set(item.kind.clone());
            active.title = Set(item.title.clone());
            active.description = Set(item.description.clone());
            active.role = Set(item.role.clone());
            active.position = Set(position);
            active.source = Set(source.to_string());
            active.file_name = Set(item.file_name.clone());
            active.extension = Set(item.extension.clone());
            active.size_bytes = Set(item.size_bytes);
            active.modified_at = Set(item.modified_at);
            active.is_valid = Set(true);
            active.invalid_reason = Set(None);
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
                root_path: Set(item.root_path),
                path: Set(item.path),
                kind: Set(item.kind),
                title: Set(item.title.clone()),
                description: Set(item.description.clone()),
                role: Set(item.role.clone()),
                position: Set(position),
                source: Set(source.to_string()),
                file_name: Set(item.file_name),
                extension: Set(item.extension),
                size_bytes: Set(item.size_bytes),
                modified_at: Set(item.modified_at),
                is_valid: Set(true),
                invalid_reason: Set(None),
                is_hidden: Set(false),
                verified_at: Set(now),
                last_checked_at: Set(Some(now)),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&txn)
            .await?
        };
        retained_ids.insert(model.id.clone());
        let association = conversation_turn_deliverable::ActiveModel {
            id: Set(uuid::Uuid::new_v4().to_string()),
            conversation_id: Set(conversation_id),
            turn_run_id: Set(turn_run_id.to_string()),
            deliverable_id: Set(model.id.clone()),
            source: Set(source.to_string()),
            title: Set(item.title),
            description: Set(item.description),
            role: Set(item.role),
            position: Set(position),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&txn)
        .await?;
        saved_pairs.push((model, association));
    }

    // A second declaration replaces only this turn. Remove aggregate rows that
    // no longer have any turn history; otherwise restore their most recent
    // remaining association as the aggregate's latest provenance.
    for deliverable_id in previous_ids.difference(&retained_ids) {
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

/// Atomically replace the complete explicit set for one in-flight turn.
pub async fn replace_declared_for_turn(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turn_run_id: &str,
    items: Vec<VerifiedDeliverable>,
) -> Result<Vec<ConversationDeliverable>, DbError> {
    replace_turn_set(
        conn,
        conversation_id,
        turn_run_id,
        SOURCE_DECLARED,
        items,
        true,
    )
    .await
}

fn inferred_extension_allowed(extension: Option<&str>) -> bool {
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
        )
    )
}

fn inference_path_allowed(path: &str) -> bool {
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
        "tmp",
        "temp",
        "tests",
        "test",
        "fixtures",
        "snapshots",
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
    let file_name = parsed
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let stem = parsed
        .file_stem()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if file_name.starts_with('.') || file_name.starts_with("~$") {
        return false;
    }
    const EXCLUDED_STEMS: &[&str] = &["draft", "preview", "temp", "tmp", "test"];
    if EXCLUDED_STEMS.contains(&stem.as_str()) {
        return false;
    }
    const EXCLUDED_MARKERS: &[&str] = &[
        ".test", "_test", "-test", ".spec", "_spec", "-spec", ".snap", "_snap", "-snap", ".tmp",
        "_tmp", "-tmp", "draft-", "draft_", "preview-", "preview_",
    ];
    !EXCLUDED_MARKERS
        .iter()
        .any(|marker| stem.starts_with(marker) || stem.ends_with(marker))
}

/// Conservative compatibility fallback based only on exclusive, finalized
/// filesystem events. It never scans assistant prose, markdown, or paths that
/// were merely read. Created output-format files are eligible; a modified path
/// is eligible only when it was already a confirmed deliverable in this same
/// conversation.
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
    if run.conversation_id != conversation_id
        || run.status != ConversationTurnRunStatus::Completed
        || run.capture_incomplete
        || run.deliverables_declared_at.is_some()
    {
        return Ok(Vec::new());
    }
    if conversation_turn_deliverable::Entity::find()
        .filter(conversation_turn_deliverable::Column::TurnRunId.eq(turn_run_id.to_string()))
        .one(conn)
        .await?
        .is_some()
    {
        return Ok(Vec::new());
    }

    let inputs = serde_json::from_str::<Vec<String>>(&run.input_paths_json)
        .unwrap_or_default()
        .into_iter()
        .map(|path| path.replace('\\', "/"))
        .collect::<HashSet<_>>();
    let changes = conversation_turn_file_change::Entity::find()
        .filter(conversation_turn_file_change::Column::TurnRunId.eq(turn_run_id.to_string()))
        .order_by_asc(conversation_turn_file_change::Column::FirstSeenAt)
        .all(conn)
        .await?;
    let known_paths = conversation_deliverable::Entity::find()
        .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
        .all(conn)
        .await?
        .into_iter()
        .map(|row| (row.root_path, row.path))
        .collect::<HashSet<_>>();

    let mut inferred = Vec::new();
    for change in changes {
        if change.source != "watcher"
            || change.attribution != "exclusive"
            || change.final_exists != Some(true)
            || inputs.contains(&change.path)
            || !inference_path_allowed(&change.path)
        {
            continue;
        }
        let extension = clean_extension(Path::new(&change.path));
        if !inferred_extension_allowed(extension.as_deref()) {
            continue;
        }
        let eligible_change = change.kind == ConversationTurnFileChangeKind::Created
            || (change.kind == ConversationTurnFileChangeKind::Modified
                && known_paths.contains(&(run.root_path.clone(), change.path.clone())));
        if !eligible_change {
            continue;
        }
        let Ok(inspected) = inspect_persisted_path(&run.root_path, &change.path, "file") else {
            continue;
        };
        inferred.push(VerifiedDeliverable {
            root_path: run.root_path.clone(),
            path: change.path,
            kind: "file".into(),
            title: inspected.file_name.clone(),
            description: None,
            role: if inferred.is_empty() {
                "primary".into()
            } else {
                "supporting".into()
            },
            file_name: inspected.file_name,
            extension: inspected.extension,
            size_bytes: inspected.size_bytes,
            modified_at: inspected.modified_at,
        });
    }

    if inferred.is_empty() {
        return Ok(Vec::new());
    }
    replace_turn_set(
        conn,
        conversation_id,
        turn_run_id,
        SOURCE_INFERRED,
        inferred,
        false,
    )
    .await
}

async fn refreshed_models(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<conversation_deliverable::Model>, DbError> {
    let rows = conversation_deliverable::Entity::find()
        .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
        .filter(conversation_deliverable::Column::IsHidden.eq(false))
        .order_by_desc(conversation_deliverable::Column::UpdatedAt)
        .all(conn)
        .await?;
    let mut refreshed = Vec::with_capacity(rows.len());
    for row in rows {
        let now = Utc::now();
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

pub async fn list_for_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<ConversationDeliverable>, DbError> {
    let models = refreshed_models(conn, conversation_id).await?;
    if models.is_empty() {
        return Ok(Vec::new());
    }
    let ids = models.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
    let associations = conversation_turn_deliverable::Entity::find()
        .filter(conversation_turn_deliverable::Column::DeliverableId.is_in(ids))
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
    let run_map = runs
        .into_iter()
        .map(|run| (run.id.clone(), run))
        .collect::<HashMap<_, _>>();
    let mut association_map: HashMap<String, conversation_turn_deliverable::Model> = HashMap::new();
    for association in associations {
        let replace = association_map
            .get(&association.deliverable_id)
            .and_then(|current| {
                Some(
                    run_map.get(&association.turn_run_id)?.started_at
                        > run_map.get(&current.turn_run_id)?.started_at,
                )
            })
            .unwrap_or(true);
        if replace {
            association_map.insert(association.deliverable_id.clone(), association);
        }
    }
    Ok(models
        .into_iter()
        .map(|model| {
            let association = association_map.get(&model.id);
            let run = association.and_then(|row| run_map.get(&row.turn_run_id));
            to_info(model, association, run)
        })
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
    Ok(associations
        .into_iter()
        .filter_map(|association| {
            model_map
                .get(&association.deliverable_id)
                .cloned()
                .map(|model| to_info(model, Some(&association), Some(&run)))
        })
        .collect())
}

pub async fn list_sets_for_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<ConversationTurnDeliverableSet>, DbError> {
    let models = refreshed_models(conn, conversation_id).await?;
    if models.is_empty() {
        return Ok(Vec::new());
    }
    let model_map = models
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect::<HashMap<_, _>>();
    let runs = conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .order_by_asc(conversation_turn_run::Column::StartedAt)
        .all(conn)
        .await?;
    let associations = conversation_turn_deliverable::Entity::find()
        .filter(conversation_turn_deliverable::Column::ConversationId.eq(conversation_id))
        .order_by_asc(conversation_turn_deliverable::Column::Position)
        .all(conn)
        .await?;
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
                })
                .collect::<Vec<_>>();
            (!deliverables.is_empty()).then_some(ConversationTurnDeliverableSet {
                turn_run_id: run.id,
                conversation_id,
                client_message_id: run.client_message_id,
                started_at: run.started_at,
                completed_at: run.completed_at,
                deliverables,
            })
        })
        .collect())
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
    use crate::db::service::artifact_service::{self, NewTurnRun, PendingFileChange};
    use crate::models::AgentType;

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
                folder_id: Some(folder_id),
                root_path: root.to_string_lossy().to_string(),
                capture_incomplete: false,
                input_paths_json: serde_json::to_string(input_paths).unwrap(),
            },
        )
        .await
        .expect("run");
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
            file_name: absolute.file_name().unwrap().to_string_lossy().to_string(),
            extension: clean_extension(&absolute),
            size_bytes: i64::try_from(metadata.len()).ok(),
            modified_at: metadata.modified().ok().map(DateTime::<Utc>::from),
        }
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

        let sets = list_sets_for_conversation(&db.conn, conversation_id)
            .await
            .unwrap();
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0].deliverables[0].title, "First version");
        assert_eq!(sets[1].deliverables[0].title, "Final version");
    }

    #[tokio::test]
    async fn explicit_empty_declaration_prevents_fallback_inference() {
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

        assert!(infer_for_turn(&db.conn, conversation_id, "run-empty")
            .await
            .unwrap()
            .is_empty());
        assert!(list_for_conversation(&db.conn, conversation_id)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn inference_uses_created_output_files_and_excludes_inputs_and_html() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input.pdf"), b"input").unwrap();
        std::fs::write(workspace.path().join("draft.html"), b"draft").unwrap();
        std::fs::write(workspace.path().join("report.test.pdf"), b"test").unwrap();
        std::fs::write(workspace.path().join("final.pdf"), b"final").unwrap();
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
            ["input.pdf", "draft.html", "report.test.pdf", "final.pdf"]
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
        assert_eq!(inferred[0].path, "final.pdf");
        assert_eq!(inferred[0].source, SOURCE_INFERRED);
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
        assert!(!listed[0].is_valid);
        assert_eq!(listed[0].invalid_reason.as_deref(), Some("file_not_found"));
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
        assert!(!listed[0].is_valid);
        assert_eq!(
            listed[0].invalid_reason.as_deref(),
            Some("unsafe_or_changed_path")
        );
    }
}
