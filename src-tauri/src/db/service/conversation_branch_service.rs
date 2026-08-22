use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set, TransactionError, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::entities::{
    conversation, conversation_branch, conversation_branch_merge, conversation_deliverable,
    conversation_turn_deliverable, conversation_turn_run,
};
use crate::db::error::DbError;
use crate::db::service::conversation_service;
use crate::models::conversation::DbConversationSummary;
use crate::models::message::{ContentBlock, ImageData, MessageTurn, TurnRole};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationBranchInfo {
    pub branch_conversation_id: i32,
    pub source_conversation_id: i32,
    pub source_title: Option<String>,
    pub source_available: bool,
    pub fork_message_id: Option<String>,
    pub fork_mode: String,
    pub source_session_id: Option<String>,
    pub branch_session_id: Option<String>,
    pub inheritance_mode: String,
    pub inherited_message_count: i32,
    pub inherited_context_chars: i64,
    pub inherited_estimated_tokens: i64,
    pub inheritance_compressed: bool,
    pub inheritance_truncated: bool,
    pub inheritance_note: Option<String>,
    pub forked_through_at: Option<chrono::DateTime<Utc>>,
    pub snapshot_version: i32,
    pub snapshot_consumed_at: Option<chrono::DateTime<Utc>>,
    pub lifecycle_state: String,
    pub lifecycle_error: Option<String>,
    pub lifecycle_updated_at: Option<chrono::DateTime<Utc>>,
    pub session_verified_at: Option<chrono::DateTime<Utc>>,
    pub first_prompt_client_message_id: Option<String>,
    pub first_prompt_queued_at: Option<chrono::DateTime<Utc>>,
    pub first_prompt_accepted_at: Option<chrono::DateTime<Utc>>,
    pub initialization_retry_count: i32,
    pub last_connection_id: Option<String>,
    pub snapshot_digest: Option<String>,
    pub created_at: chrono::DateTime<Utc>,
    pub last_merged_at: Option<chrono::DateTime<Utc>>,
    pub merge_target_conversation_id: Option<i32>,
}

impl From<conversation_branch::Model> for ConversationBranchInfo {
    fn from(value: conversation_branch::Model) -> Self {
        Self {
            branch_conversation_id: value.branch_conversation_id,
            source_conversation_id: value.source_conversation_id,
            source_title: value.source_title,
            source_available: true,
            fork_message_id: value.fork_message_id,
            fork_mode: value.fork_mode,
            source_session_id: value.source_session_id,
            branch_session_id: value.branch_session_id,
            inheritance_mode: value.inheritance_mode,
            inherited_message_count: value.inherited_message_count,
            inherited_context_chars: value.inherited_context_chars,
            inherited_estimated_tokens: value.inherited_estimated_tokens,
            inheritance_compressed: value.inheritance_compressed,
            inheritance_truncated: value.inheritance_truncated,
            inheritance_note: value.inheritance_note,
            forked_through_at: value.forked_through_at,
            snapshot_version: value.snapshot_version,
            snapshot_consumed_at: value.snapshot_consumed_at,
            lifecycle_state: value.lifecycle_state,
            lifecycle_error: value.lifecycle_error,
            lifecycle_updated_at: value.lifecycle_updated_at,
            session_verified_at: value.session_verified_at,
            first_prompt_client_message_id: value.first_prompt_client_message_id,
            first_prompt_queued_at: value.first_prompt_queued_at,
            first_prompt_accepted_at: value.first_prompt_accepted_at,
            initialization_retry_count: value.initialization_retry_count,
            last_connection_id: value.last_connection_id,
            snapshot_digest: value.snapshot_digest,
            created_at: value.created_at,
            last_merged_at: value.last_merged_at,
            merge_target_conversation_id: value.merge_target_conversation_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BranchInheritanceRecord {
    pub source_session_id: Option<String>,
    pub branch_session_id: Option<String>,
    pub inheritance_mode: String,
    pub inherited_message_count: i32,
    pub inherited_context_chars: i64,
    pub inherited_estimated_tokens: i64,
    pub inheritance_compressed: bool,
    pub inheritance_truncated: bool,
    pub inheritance_note: Option<String>,
    pub forked_through_at: Option<chrono::DateTime<Utc>>,
    pub snapshot_version: i32,
    pub snapshot_context: Option<String>,
    pub snapshot_images: Vec<ImageData>,
}

pub async fn create_branch_row(
    conn: &DatabaseConnection,
    source: &DbConversationSummary,
    external_id: Option<String>,
    fork_message_id: Option<String>,
    fork_mode: &str,
    inheritance: BranchInheritanceRecord,
) -> Result<(conversation::Model, ConversationBranchInfo), DbError> {
    let title = Some(format!(
        "{} · 分支",
        source.title.as_deref().unwrap_or("未命名会话")
    ));
    let created = if source.kind == conversation::ConversationKind::Chat {
        conversation_service::create_chat(
            conn,
            source.folder_id,
            source.agent_type,
            title,
            source.git_branch.clone(),
        )
        .await?
    } else {
        conversation_service::create(
            conn,
            source.folder_id,
            source.agent_type,
            title,
            source.git_branch.clone(),
        )
        .await?
    };

    let created_id = created.id;
    let mut active = created.into_active_model();
    active.title_locked = Set(true);
    active.model = Set(source.model.clone());
    active.external_id = Set(external_id);
    active.origin_cwd = Set(source.origin_cwd.clone());
    // A branch with no user turn is idle, not running. In particular this
    // prevents a transient ACP disconnect from being interpreted as a user
    // cancellation before the first prompt exists.
    active.status = Set(conversation::ConversationStatus::PendingReview);
    let created = match active.update(conn).await {
        Ok(created) => created,
        Err(error) => {
            let _ = remove_incomplete_branch(conn, created_id).await;
            return Err(DbError::Database(error));
        }
    };
    let now = Utc::now();
    let provisional = fork_mode == "snapshot" && inheritance.snapshot_context.is_some();
    let snapshot_digest = inheritance.snapshot_context.as_deref().map(|context| {
        let mut digest = Sha256::new();
        digest.update(context.as_bytes());
        format!("{:x}", digest.finalize())
    });
    let relation = conversation_branch::ActiveModel {
        branch_conversation_id: Set(created.id),
        source_conversation_id: Set(source.id),
        source_title: Set(source.title.clone()),
        fork_message_id: Set(fork_message_id),
        fork_mode: Set(fork_mode.to_string()),
        source_session_id: Set(inheritance.source_session_id),
        branch_session_id: Set(inheritance.branch_session_id),
        inheritance_mode: Set(inheritance.inheritance_mode),
        inherited_message_count: Set(inheritance.inherited_message_count),
        inherited_context_chars: Set(inheritance.inherited_context_chars),
        inherited_estimated_tokens: Set(inheritance.inherited_estimated_tokens),
        inheritance_compressed: Set(inheritance.inheritance_compressed),
        inheritance_truncated: Set(inheritance.inheritance_truncated),
        inheritance_note: Set(inheritance.inheritance_note),
        forked_through_at: Set(inheritance.forked_through_at),
        snapshot_version: Set(inheritance.snapshot_version),
        snapshot_images_json: Set((!inheritance.snapshot_images.is_empty()).then(|| {
            serde_json::to_string(&inheritance.snapshot_images).unwrap_or_else(|_| "[]".into())
        })),
        snapshot_context: Set(inheritance.snapshot_context),
        snapshot_consumed_at: Set(None),
        lifecycle_state: Set(if provisional {
            "provisional".into()
        } else {
            "ready".into()
        }),
        lifecycle_error: Set(None),
        lifecycle_updated_at: Set(Some(now)),
        session_verified_at: Set((!provisional).then_some(now)),
        first_prompt_client_message_id: Set(None),
        first_prompt_queued_at: Set(None),
        first_prompt_accepted_at: Set(None),
        initialization_retry_count: Set(0),
        last_connection_id: Set(None),
        snapshot_digest: Set(snapshot_digest),
        created_at: Set(now),
        last_merged_at: Set(None),
        last_merge_key: Set(None),
        merge_target_conversation_id: Set(None),
    }
    .insert(conn)
    .await;
    let relation = match relation {
        Ok(relation) => relation,
        Err(error) => {
            let _ = remove_incomplete_branch(conn, created.id).await;
            return Err(DbError::Database(error));
        }
    };
    Ok((created, relation.into()))
}

pub async fn update_branch_session_id(
    conn: &DatabaseConnection,
    conversation_id: i32,
    session_id: String,
) -> Result<(), DbError> {
    if let Some(row) = conversation_branch::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    {
        let mut active = row.into_active_model();
        active.branch_session_id = Set(Some(session_id));
        active.update(conn).await?;
    }
    Ok(())
}

pub const PROVISIONAL_STATES: &[&str] = &[
    "snapshot_ready",
    "provisional",
    "session_creating",
    "connection_ready",
    "prompt_ready",
    "retryable_failed",
];

/// Runtime compatibility repair for branches created by builds that persisted
/// session/new's ephemeral id before any prompt created a durable rollout.
/// The shape is deliberately strict: an unconsumed snapshot, zero parsed
/// messages and no turn-run receipt. Anything with user work is left alone.
pub async fn repair_empty_snapshot_as_provisional(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<bool, DbError> {
    let Some(branch) = conversation_branch::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    if branch.fork_mode != "snapshot"
        || branch.snapshot_consumed_at.is_some()
        || branch
            .snapshot_context
            .as_deref()
            .is_none_or(|context| context.trim().is_empty())
    {
        return Ok(false);
    }
    let Some(conversation) = conversation::Entity::find_by_id(conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    if branch.branch_session_id.is_none()
        && conversation.external_id.is_none()
        && conversation.status == conversation::ConversationStatus::PendingReview
        && PROVISIONAL_STATES.contains(&branch.lifecycle_state.as_str())
    {
        return Ok(false);
    }
    if conversation.message_count != 0
        || conversation_turn_run::Entity::find()
            .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
            .one(conn)
            .await?
            .is_some()
    {
        return Ok(false);
    }

    let now = Utc::now();
    let prior_external_id = conversation.external_id.clone();
    let prior_external_id_for_tx = prior_external_id.clone();
    let result = conn
        .transaction::<_, (), sea_orm::DbErr>(|txn| {
            Box::pin(async move {
                let mut branch_active = branch.into_active_model();
                branch_active.branch_session_id = Set(None);
                branch_active.lifecycle_state = Set("provisional".into());
                branch_active.lifecycle_error = Set(prior_external_id_for_tx.as_ref().map(|_| {
                    "The previous empty session was not durable; it will be recreated on first use."
                        .into()
                }));
                branch_active.lifecycle_updated_at = Set(Some(now));
                branch_active.session_verified_at = Set(None);
                branch_active.last_connection_id = Set(None);
                branch_active.update(txn).await?;

                let mut conversation_active = conversation.into_active_model();
                conversation_active.external_id = Set(None);
                conversation_active.status = Set(conversation::ConversationStatus::PendingReview);
                conversation_active.updated_at = Set(now);
                conversation_active.update(txn).await?;
                Ok(())
            })
        })
        .await;
    match result {
        Ok(()) => {
            tracing::warn!(
                branch_conversation_id = conversation_id,
                lifecycle_state = "provisional",
                prior_external_session_id = ?prior_external_id,
                snapshot_consumed_at = ?Option::<chrono::DateTime<Utc>>::None,
                failure_classification = "missing_durable_session",
                "[ACP][branch] repaired empty snapshot branch as provisional"
            );
            Ok(true)
        }
        Err(TransactionError::Connection(error) | TransactionError::Transaction(error)) => {
            Err(DbError::Database(error))
        }
    }
}

pub async fn is_provisional_snapshot(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<bool, DbError> {
    let Some(branch) = conversation_branch::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    Ok(branch.fork_mode == "snapshot"
        && branch.snapshot_consumed_at.is_none()
        && PROVISIONAL_STATES.contains(&branch.lifecycle_state.as_str()))
}

pub async fn mark_initialization_state(
    conn: &DatabaseConnection,
    conversation_id: i32,
    lifecycle_state: &str,
    connection_id: Option<String>,
    error: Option<String>,
    increment_retry: bool,
) -> Result<(), DbError> {
    let Some(row) = conversation_branch::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(());
    };
    if row.snapshot_consumed_at.is_some() {
        return Ok(());
    }
    // SessionStarted is handled by a DB subscriber and can arrive after the
    // synchronous restore path already reached prompt_ready (or failed). Do
    // not let that delayed lower-level event move the durable state backward.
    if lifecycle_state == "connection_ready"
        && !matches!(
            row.lifecycle_state.as_str(),
            "snapshot_ready" | "provisional" | "session_creating" | "connection_ready"
        )
    {
        return Ok(());
    }
    let retry_count = row.initialization_retry_count;
    let mut active = row.into_active_model();
    active.lifecycle_state = Set(lifecycle_state.to_string());
    active.lifecycle_error = Set(error);
    active.lifecycle_updated_at = Set(Some(Utc::now()));
    active.last_connection_id = Set(connection_id);
    if increment_retry {
        active.initialization_retry_count = Set(retry_count.saturating_add(1));
    }
    active.update(conn).await?;
    Ok(())
}

pub async fn mark_first_prompt_queued(
    conn: &DatabaseConnection,
    conversation_id: i32,
    client_message_id: Option<&str>,
    connection_id: &str,
) -> Result<(), DbError> {
    let Some(row) = conversation_branch::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(());
    };
    if row.snapshot_consumed_at.is_some() {
        return Ok(());
    }
    let has_client_message_id = row.first_prompt_client_message_id.is_some();
    let has_queued_at = row.first_prompt_queued_at.is_some();
    let now = Utc::now();
    let mut active = row.into_active_model();
    if !has_client_message_id {
        active.first_prompt_client_message_id = Set(client_message_id.map(str::to_owned));
    }
    if !has_queued_at {
        active.first_prompt_queued_at = Set(Some(now));
    }
    active.lifecycle_state = Set("prompt_ready".into());
    active.lifecycle_error = Set(None);
    active.lifecycle_updated_at = Set(Some(now));
    active.last_connection_id = Set(Some(connection_id.to_string()));
    active.update(conn).await?;
    Ok(())
}

/// Commit the point at which the private snapshot and the first real user
/// prompt have both entered the target connection. Until this transaction
/// succeeds the session id remains provisional and the snapshot remains
/// retryable; after it succeeds the normal restore path owns the branch.
pub async fn finalize_first_prompt(
    conn: &DatabaseConnection,
    conversation_id: i32,
    session_id: &str,
    connection_id: &str,
    client_message_id: Option<&str>,
) -> Result<(), DbError> {
    let now = Utc::now();
    let session_id = session_id.to_string();
    let connection_id = connection_id.to_string();
    let client_message_id = client_message_id.map(str::to_owned);
    let result = conn
        .transaction::<_, (), sea_orm::DbErr>(|txn| {
            Box::pin(async move {
                let Some(branch) = conversation_branch::Entity::find_by_id(conversation_id)
                    .one(txn)
                    .await?
                else {
                    return Ok(());
                };
                if branch.snapshot_consumed_at.is_some() {
                    return Ok(());
                }
                let mut active = branch.into_active_model();
                active.branch_session_id = Set(Some(session_id.clone()));
                active.snapshot_consumed_at = Set(Some(now));
                active.lifecycle_state = Set("ready".into());
                active.lifecycle_error = Set(None);
                active.lifecycle_updated_at = Set(Some(now));
                active.session_verified_at = Set(Some(now));
                active.first_prompt_client_message_id = Set(client_message_id.clone());
                active.first_prompt_queued_at = Set(Some(now));
                active.first_prompt_accepted_at = Set(Some(now));
                active.last_connection_id = Set(Some(connection_id));
                active.update(txn).await?;

                let Some(conversation) = conversation::Entity::find_by_id(conversation_id)
                    .one(txn)
                    .await?
                else {
                    return Ok(());
                };
                let mut conversation_active = conversation.into_active_model();
                conversation_active.external_id = Set(Some(session_id));
                conversation_active.updated_at = Set(now);
                conversation_active.update(txn).await?;
                Ok(())
            })
        })
        .await;
    match result {
        Ok(()) => Ok(()),
        Err(TransactionError::Connection(error) | TransactionError::Transaction(error)) => {
            Err(DbError::Database(error))
        }
    }
}

pub async fn was_first_prompt_accepted(
    conn: &DatabaseConnection,
    conversation_id: i32,
    client_message_id: &str,
) -> Result<bool, DbError> {
    Ok(conversation_branch::Entity::find_by_id(conversation_id)
        .filter(
            conversation_branch::Column::FirstPromptClientMessageId
                .eq(client_message_id.to_string()),
        )
        .filter(conversation_branch::Column::FirstPromptAcceptedAt.is_not_null())
        .one(conn)
        .await?
        .is_some())
}

/// Hard-delete a branch row that was created during the current request but
/// could not be bound to its freshly-created ACP session. This is deliberately
/// narrower than user-facing conversation deletion: the row has never been
/// published or opened and cannot yet own messages, turns, or deliverables.
/// Keeping the relation + conversation deletion in one transaction prevents a
/// failed create from leaving either half behind.
pub async fn remove_incomplete_branch(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<(), DbError> {
    let result = conn
        .transaction::<_, (), sea_orm::DbErr>(|txn| {
            Box::pin(async move {
                conversation_branch::Entity::delete_by_id(conversation_id)
                    .exec(txn)
                    .await?;
                conversation::Entity::delete_by_id(conversation_id)
                    .exec(txn)
                    .await?;
                Ok(())
            })
        })
        .await;
    match result {
        Ok(()) => Ok(()),
        Err(TransactionError::Connection(error) | TransactionError::Transaction(error)) => {
            Err(DbError::Database(error))
        }
    }
}

pub async fn get_info(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Option<ConversationBranchInfo>, DbError> {
    let Some(row) = conversation_branch::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(None);
    };
    let source = conversation::Entity::find_by_id(row.source_conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?;
    let mut info: ConversationBranchInfo = row.into();
    info.source_available = source.is_some();
    if let Some(source) = source {
        info.source_title = source.title;
    }
    Ok(Some(info))
}

/// Backfill the durable branch relation omitted by the original Fork & Send
/// implementation. The caller supplies the parent session read from Codex's
/// own rollout header; that protocol-level identity is safer than guessing from
/// titles or timestamps. The additional `[Fork]`/root checks prevent imported
/// delegate sessions that also carry `parent_thread_id` from becoming
/// user-created conversation branches.
pub async fn repair_native_fork_relation(
    conn: &DatabaseConnection,
    conversation_id: i32,
    parent_session_id: &str,
) -> Result<Option<ConversationBranchInfo>, DbError> {
    if let Some(info) = get_info(conn, conversation_id).await? {
        return Ok(Some(info));
    }
    let Some(branch) = conversation::Entity::find_by_id(conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?
    else {
        return Ok(None);
    };
    let branch_session_id = branch
        .external_id
        .as_deref()
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty());
    if branch.agent_type != "codex"
        || branch.parent_id.is_some()
        || branch_session_id.is_none()
        || branch
            .title
            .as_deref()
            .is_none_or(|title| !title.trim_start().starts_with("[Fork]"))
    {
        return Ok(None);
    }
    let parent_session_id = parent_session_id.trim();
    if parent_session_id.is_empty() || Some(parent_session_id) == branch_session_id {
        return Ok(None);
    }
    let Some(source) = conversation::Entity::find()
        .filter(conversation::Column::ExternalId.eq(parent_session_id))
        .filter(conversation::Column::FolderId.eq(branch.folder_id))
        .filter(conversation::Column::AgentType.eq(branch.agent_type.clone()))
        .filter(conversation::Column::ParentId.is_null())
        .filter(conversation::Column::DeletedAt.is_null())
        .filter(conversation::Column::Id.ne(conversation_id))
        .order_by_desc(conversation::Column::CreatedAt)
        .one(conn)
        .await?
    else {
        return Ok(None);
    };
    let now = Utc::now();
    let relation = conversation_branch::ActiveModel {
        branch_conversation_id: Set(conversation_id),
        source_conversation_id: Set(source.id),
        source_title: Set(source.title.clone()),
        fork_message_id: Set(None),
        fork_mode: Set("native".into()),
        source_session_id: Set(Some(parent_session_id.to_string())),
        branch_session_id: Set(branch.external_id.clone()),
        inheritance_mode: Set("native_fork".into()),
        inherited_message_count: Set(source.message_count),
        inherited_context_chars: Set(0),
        inherited_estimated_tokens: Set(0),
        inheritance_compressed: Set(false),
        inheritance_truncated: Set(false),
        inheritance_note: Set(Some(
            "Repaired from Codex session_meta.parent_thread_id; the complete native session context was inherited."
                .into(),
        )),
        forked_through_at: Set(Some(source.created_at)),
        snapshot_version: Set(2),
        snapshot_images_json: Set(None),
        snapshot_context: Set(None),
        snapshot_consumed_at: Set(None),
        lifecycle_state: Set("ready".into()),
        lifecycle_error: Set(None),
        lifecycle_updated_at: Set(Some(now)),
        session_verified_at: Set(Some(now)),
        first_prompt_client_message_id: Set(None),
        first_prompt_queued_at: Set(None),
        first_prompt_accepted_at: Set(None),
        initialization_retry_count: Set(0),
        last_connection_id: Set(None),
        snapshot_digest: Set(None),
        created_at: Set(now),
        last_merged_at: Set(None),
        last_merge_key: Set(None),
        merge_target_conversation_id: Set(None),
    }
    .insert(conn)
    .await;
    if let Err(error) = relation {
        if let Some(info) = get_info(conn, conversation_id).await? {
            return Ok(Some(info));
        }
        return Err(DbError::Database(error));
    }
    tracing::info!(
        branch_conversation_id = conversation_id,
        source_conversation_id = source.id,
        inheritance_mode = "native_fork",
        repair = true,
        "[ACP][branch] repaired legacy fork-send relation from Codex rollout metadata"
    );
    get_info(conn, conversation_id).await
}

#[derive(Debug, Clone)]
pub struct PendingBranchSnapshot {
    pub context: String,
    pub images: Vec<ImageData>,
}

pub async fn pending_snapshot(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Option<PendingBranchSnapshot>, DbError> {
    let row = conversation_branch::Entity::find_by_id(conversation_id)
        .filter(conversation_branch::Column::SnapshotConsumedAt.is_null())
        .one(conn)
        .await?;
    Ok(row.and_then(|row| {
        let context = row.snapshot_context?.trim().to_string();
        if context.is_empty() {
            return None;
        }
        let images = row
            .snapshot_images_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();
        Some(PendingBranchSnapshot { context, images })
    }))
}

/// Whether it is safe to replace this branch's ACP session during restore.
/// The replacement is lossless only before the inheritance snapshot or any
/// user message has reached the old session. This also repairs branches made
/// by older builds that persisted a session id before session/new was usable.
pub async fn can_reinitialize_empty_snapshot_session(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<bool, DbError> {
    let Some(branch) = conversation_branch::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    if branch.fork_mode != "snapshot"
        || branch.snapshot_consumed_at.is_some()
        || branch
            .snapshot_context
            .as_deref()
            .is_none_or(|context| context.trim().is_empty())
    {
        return Ok(false);
    }
    let conversation = conversation::Entity::find_by_id(conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?;
    Ok(conversation.is_some_and(|conversation| conversation.message_count == 0))
}

pub async fn mark_snapshot_consumed(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<(), DbError> {
    if let Some(row) = conversation_branch::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    {
        let mut active = row.into_active_model();
        active.snapshot_consumed_at = Set(Some(Utc::now()));
        active.update(conn).await?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeBranchResult {
    pub merge_id: String,
    pub target_conversation_id: i32,
    pub copied_deliverable_count: usize,
    pub deduplicated: bool,
}

pub async fn merge_branch(
    conn: &DatabaseConnection,
    branch_conversation_id: i32,
    request_id: String,
    summary: String,
    deliverable_ids: Vec<String>,
) -> Result<MergeBranchResult, DbError> {
    let request_id = request_id.trim().to_string();
    if request_id.is_empty() || summary.trim().is_empty() {
        return Err(DbError::Validation(
            "request_id and merge summary are required".into(),
        ));
    }
    if let Some(existing) = conversation_branch_merge::Entity::find_by_id(&request_id)
        .one(conn)
        .await?
    {
        let copied = serde_json::from_str::<Vec<String>>(&existing.deliverable_ids_json)
            .unwrap_or_default()
            .len();
        return Ok(MergeBranchResult {
            merge_id: existing.id,
            target_conversation_id: existing.target_conversation_id,
            copied_deliverable_count: copied,
            deduplicated: true,
        });
    }
    let relation = conversation_branch::Entity::find_by_id(branch_conversation_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound("conversation branch".into()))?;
    // Deletion is soft and relations intentionally do not cascade. Reject a
    // merge when the source is gone; deleting either side never deletes the
    // other side or the audit row.
    conversation::Entity::find_by_id(relation.source_conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound("source conversation".into()))?;

    let selected = if deliverable_ids.is_empty() {
        Vec::new()
    } else {
        conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(branch_conversation_id))
            .filter(conversation_deliverable::Column::Id.is_in(deliverable_ids.clone()))
            .filter(conversation_deliverable::Column::IsValid.eq(true))
            .filter(conversation_deliverable::Column::IsHidden.eq(false))
            .all(conn)
            .await?
    };
    let copied_count = selected.len();
    let copied_source_ids: Vec<String> = selected.iter().map(|d| d.id.clone()).collect();
    let now = Utc::now();
    let target_id = relation.source_conversation_id;
    let run_id = format!("branch-merge-{request_id}");
    let client_message_id = run_id.clone();
    let root_path = selected
        .first()
        .map(|item| item.root_path.clone())
        .unwrap_or_default();
    let summary_for_tx = summary.trim().to_string();
    let request_for_tx = request_id.clone();
    let transaction_result = conn
        .transaction::<_, (), sea_orm::DbErr>(|txn| {
            Box::pin(async move {
                conversation_branch_merge::ActiveModel {
                    id: Set(request_for_tx.clone()),
                    branch_conversation_id: Set(branch_conversation_id),
                    source_conversation_id: Set(target_id),
                    target_conversation_id: Set(target_id),
                    summary: Set(summary_for_tx),
                    deliverable_ids_json: Set(
                        serde_json::to_string(&copied_source_ids).unwrap_or_else(|_| "[]".into())
                    ),
                    created_at: Set(now),
                    context_consumed_at: Set(None),
                }
                .insert(txn)
                .await?;
                conversation_turn_run::ActiveModel {
                    id: Set(run_id.clone()),
                    conversation_id: Set(target_id),
                    connection_id: Set(format!("branch-merge:{branch_conversation_id}")),
                    client_message_id: Set(Some(client_message_id)),
                    prompt_accepted_at: Set(Some(now)),
                    prompt_fingerprint: Set(None),
                    cancel_request_id: Set(None),
                    cancel_requested_at: Set(None),
                    cancel_deadline_at: Set(None),
                    folder_id: Set(None),
                    root_path: Set(root_path),
                    status: Set(conversation_turn_run::ConversationTurnRunStatus::Completed),
                    capture_incomplete: Set(false),
                    stop_reason: Set(Some("branch_merge".into())),
                    started_at: Set(now),
                    completed_at: Set(Some(now)),
                    deliverables_declared_at: Set((!selected.is_empty()).then_some(now)),
                    input_paths_json: Set("[]".into()),
                    declaration_status: Set(if selected.is_empty() {
                        "success_empty".into()
                    } else {
                        "success".into()
                    }),
                    declaration_attempted_at: Set(Some(now)),
                    declaration_error: Set(None),
                    expectation_json: Set("{}".into()),
                    settlement_status: Set("settled".into()),
                    settled_at: Set(Some(now)),
                    missing_expected_paths_json: Set("[]".into()),
                }
                .insert(txn)
                .await?;
                for (position, source) in selected.into_iter().enumerate() {
                    let copied_id = uuid::Uuid::new_v4().to_string();
                    conversation_deliverable::ActiveModel {
                        id: Set(copied_id.clone()),
                        conversation_id: Set(target_id),
                        turn_run_id: Set(Some(run_id.clone())),
                        root_path: Set(source.root_path),
                        path: Set(source.path),
                        kind: Set(source.kind),
                        title: Set(source.title.clone()),
                        description: Set(source.description),
                        role: Set(source.role.clone()),
                        category: Set(source.category.clone()),
                        change_kind: Set(source.change_kind.clone()),
                        position: Set(position as i32),
                        source: Set("branch_merge".into()),
                        file_name: Set(source.file_name),
                        extension: Set(source.extension),
                        size_bytes: Set(source.size_bytes),
                        modified_at: Set(source.modified_at),
                        is_valid: Set(source.is_valid),
                        invalid_reason: Set(source.invalid_reason),
                        is_hidden: Set(false),
                        verified_at: Set(source.verified_at),
                        last_checked_at: Set(source.last_checked_at),
                        created_at: Set(now),
                        updated_at: Set(now),
                    }
                    .insert(txn)
                    .await?;
                    conversation_turn_deliverable::ActiveModel {
                        id: Set(uuid::Uuid::new_v4().to_string()),
                        conversation_id: Set(target_id),
                        turn_run_id: Set(run_id.clone()),
                        deliverable_id: Set(copied_id),
                        source: Set("branch_merge".into()),
                        title: Set(source.title),
                        description: Set(None),
                        role: Set(source.role),
                        category: Set(source.category),
                        change_kind: Set(source.change_kind),
                        position: Set(position as i32),
                        created_at: Set(now),
                        updated_at: Set(now),
                    }
                    .insert(txn)
                    .await?;
                }
                let mut active = relation.into_active_model();
                active.last_merged_at = Set(Some(now));
                active.last_merge_key = Set(Some(request_for_tx));
                active.merge_target_conversation_id = Set(Some(target_id));
                active.update(txn).await?;
                Ok(())
            })
        })
        .await;
    if let Err(error) = transaction_result {
        // Two identical merge clicks can both pass the optimistic pre-read.
        // The primary key is the final arbiter; the loser re-reads the winner
        // and reports a deduplicated success instead of surfacing SQLITE UNIQUE.
        if let Some(existing) = conversation_branch_merge::Entity::find_by_id(&request_id)
            .one(conn)
            .await?
        {
            let copied = serde_json::from_str::<Vec<String>>(&existing.deliverable_ids_json)
                .unwrap_or_default()
                .len();
            return Ok(MergeBranchResult {
                merge_id: existing.id,
                target_conversation_id: existing.target_conversation_id,
                copied_deliverable_count: copied,
                deduplicated: true,
            });
        }
        return Err(match error {
            TransactionError::Connection(error) | TransactionError::Transaction(error) => {
                DbError::Database(error)
            }
        });
    }

    Ok(MergeBranchResult {
        merge_id: request_id,
        target_conversation_id: target_id,
        copied_deliverable_count: copied_count,
        deduplicated: false,
    })
}

pub async fn merge_turns_for_target(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<MessageTurn>, DbError> {
    let rows = conversation_branch_merge::Entity::find()
        .filter(conversation_branch_merge::Column::TargetConversationId.eq(conversation_id))
        .order_by_asc(conversation_branch_merge::Column::CreatedAt)
        .all(conn)
        .await?;
    let mut turns = Vec::with_capacity(rows.len() * 2);
    for row in rows {
        let user_id = format!("branch-merge-{}", row.id);
        turns.push(MessageTurn {
            id: user_id,
            role: TurnRole::User,
            blocks: vec![ContentBlock::Text {
                text: format!("合并分支 #{} 的成果", row.branch_conversation_id),
            }],
            timestamp: row.created_at,
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: Some(row.created_at),
        });
        turns.push(MessageTurn {
            id: format!("branch-merge-{}-result", row.id),
            role: TurnRole::Assistant,
            blocks: vec![ContentBlock::Text { text: row.summary }],
            timestamp: row.created_at,
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: Some(row.created_at),
        });
    }
    Ok(turns)
}

pub async fn pending_merge_context(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<(String, String)>, DbError> {
    let rows = conversation_branch_merge::Entity::find()
        .filter(conversation_branch_merge::Column::TargetConversationId.eq(conversation_id))
        .filter(conversation_branch_merge::Column::ContextConsumedAt.is_null())
        .order_by_asc(conversation_branch_merge::Column::CreatedAt)
        .all(conn)
        .await?;
    let mut contexts = Vec::with_capacity(rows.len());
    for row in rows {
        let run_id = format!("branch-merge-{}", row.id);
        let files = conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
            .filter(conversation_deliverable::Column::TurnRunId.eq(run_id))
            .filter(conversation_deliverable::Column::IsValid.eq(true))
            .filter(conversation_deliverable::Column::IsHidden.eq(false))
            .order_by_asc(conversation_deliverable::Column::Position)
            .all(conn)
            .await?;
        let mut text = format!(
            "Merged from user branch #{} at {}:\n{}",
            row.branch_conversation_id,
            row.created_at.to_rfc3339(),
            row.summary
        );
        if !files.is_empty() {
            text.push_str("\nSelected deliverables:");
            for file in files {
                text.push_str(&format!("\n- {}", file.path));
            }
        }
        contexts.push((row.id, text));
    }
    Ok(contexts)
}

pub async fn mark_merge_context_consumed(
    conn: &DatabaseConnection,
    conversation_id: i32,
    merge_ids: &[String],
) -> Result<(), DbError> {
    if merge_ids.is_empty() {
        return Ok(());
    }
    conversation_branch_merge::Entity::update_many()
        .col_expr(
            conversation_branch_merge::Column::ContextConsumedAt,
            sea_orm::sea_query::Expr::value(Utc::now()),
        )
        .filter(conversation_branch_merge::Column::TargetConversationId.eq(conversation_id))
        .filter(conversation_branch_merge::Column::Id.is_in(merge_ids.to_vec()))
        .filter(conversation_branch_merge::Column::ContextConsumedAt.is_null())
        .exec(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::models::AgentType;

    #[tokio::test]
    async fn legacy_fork_send_relation_repairs_from_native_parent_identity() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-native-fork-repair").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        conversation_service::bind_external_id(&db.conn, source_id, "session-parent", &[])
            .await
            .unwrap();
        let branch_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let branch = conversation::Entity::find_by_id(branch_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        let mut active = branch.into_active_model();
        active.title = Set(Some("[Fork] Topic".into()));
        active.title_locked = Set(true);
        active.external_id = Set(Some("session-child".into()));
        active.update(&db.conn).await.unwrap();

        let repaired = repair_native_fork_relation(&db.conn, branch_id, "session-parent")
            .await
            .unwrap()
            .expect("legacy fork relation");
        assert_eq!(repaired.source_conversation_id, source_id);
        assert_eq!(repaired.inheritance_mode, "native_fork");
        assert!(!repaired.inheritance_compressed);
        assert_eq!(
            repair_native_fork_relation(&db.conn, branch_id, "session-parent")
                .await
                .unwrap()
                .unwrap()
                .source_conversation_id,
            source_id,
            "repair is idempotent"
        );
    }

    #[tokio::test]
    async fn snapshot_branch_persists_relation_without_delegate_parentage() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-branch-test").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let source = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();

        let (branch, info) = create_branch_row(
            &db.conn,
            &source,
            None,
            Some("turn-3".into()),
            "snapshot",
            BranchInheritanceRecord {
                source_session_id: source.external_id.clone(),
                branch_session_id: None,
                inheritance_mode: "full_replay".into(),
                inherited_message_count: 1,
                inherited_context_chars: 11,
                inherited_estimated_tokens: 3,
                inheritance_compressed: false,
                inheritance_truncated: false,
                inheritance_note: None,
                forked_through_at: None,
                snapshot_version: 2,
                snapshot_context: Some("User: hello".into()),
                snapshot_images: Vec::new(),
            },
        )
        .await
        .unwrap();

        assert_ne!(branch.id, source_id);
        assert_eq!(branch.folder_id, folder_id);
        assert_eq!(branch.parent_id, None);
        assert_eq!(branch.kind, conversation::ConversationKind::Regular);
        assert_eq!(info.source_conversation_id, source_id);
        assert!(info.source_available);
        assert_eq!(info.fork_message_id.as_deref(), Some("turn-3"));
        assert_eq!(info.lifecycle_state, "provisional");
        assert!(info.branch_session_id.is_none());
        assert_eq!(branch.external_id, None);
        assert_eq!(
            branch.status,
            conversation::ConversationStatus::PendingReview
        );
        assert_eq!(
            pending_snapshot(&db.conn, branch.id)
                .await
                .unwrap()
                .map(|snapshot| snapshot.context),
            Some("User: hello".into())
        );
        assert!(can_reinitialize_empty_snapshot_session(&db.conn, branch.id)
            .await
            .unwrap());
        mark_first_prompt_queued(&db.conn, branch.id, Some("optimistic-first"), "conn-first")
            .await
            .unwrap();
        mark_initialization_state(
            &db.conn,
            branch.id,
            "connection_ready",
            Some("conn-first".into()),
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            get_info(&db.conn, branch.id)
                .await
                .unwrap()
                .unwrap()
                .lifecycle_state,
            "prompt_ready",
            "a delayed SessionStarted event must not regress prompt readiness"
        );
        finalize_first_prompt(
            &db.conn,
            branch.id,
            "real-session",
            "conn-first",
            Some("optimistic-first"),
        )
        .await
        .unwrap();
        assert!(pending_snapshot(&db.conn, branch.id)
            .await
            .unwrap()
            .is_none());
        assert!(
            !can_reinitialize_empty_snapshot_session(&db.conn, branch.id)
                .await
                .unwrap()
        );
        let promoted = get_info(&db.conn, branch.id).await.unwrap().unwrap();
        assert_eq!(promoted.lifecycle_state, "ready");
        assert_eq!(promoted.branch_session_id.as_deref(), Some("real-session"));
        assert_eq!(
            promoted.first_prompt_client_message_id.as_deref(),
            Some("optimistic-first")
        );
        assert!(promoted.first_prompt_accepted_at.is_some());
        assert!(
            was_first_prompt_accepted(&db.conn, branch.id, "optimistic-first")
                .await
                .unwrap()
        );
        assert!(
            !was_first_prompt_accepted(&db.conn, branch.id, "different-id")
                .await
                .unwrap()
        );
        assert_eq!(
            conversation_service::get_by_id(&db.conn, branch.id)
                .await
                .unwrap()
                .external_id
                .as_deref(),
            Some("real-session")
        );

        conversation_service::soft_delete(&db.conn, source_id)
            .await
            .unwrap();
        let info = get_info(&db.conn, branch.id).await.unwrap().unwrap();
        assert!(!info.source_available);
        assert!(conversation_service::get_by_id(&db.conn, branch.id)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn damaged_empty_snapshot_branch_is_repaired_without_losing_snapshot() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-branch-repair-test").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let source = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();
        let (branch, _) = create_branch_row(
            &db.conn,
            &source,
            None,
            None,
            "snapshot",
            BranchInheritanceRecord {
                source_session_id: None,
                branch_session_id: None,
                inheritance_mode: "structured_snapshot".into(),
                inherited_message_count: 12,
                inherited_context_chars: 42,
                inherited_estimated_tokens: 11,
                inheritance_compressed: true,
                inheritance_truncated: false,
                inheritance_note: None,
                forked_through_at: None,
                snapshot_version: 2,
                snapshot_context: Some("important context".into()),
                snapshot_images: Vec::new(),
            },
        )
        .await
        .unwrap();
        conversation_service::bind_external_id(&db.conn, branch.id, "fake-session", &[])
            .await
            .unwrap();
        update_branch_session_id(&db.conn, branch.id, "fake-session".into())
            .await
            .unwrap();
        conversation_service::update_status(
            &db.conn,
            branch.id,
            conversation::ConversationStatus::Cancelled,
        )
        .await
        .unwrap();

        assert!(repair_empty_snapshot_as_provisional(&db.conn, branch.id)
            .await
            .unwrap());
        let repaired = get_info(&db.conn, branch.id).await.unwrap().unwrap();
        assert_eq!(repaired.lifecycle_state, "provisional");
        assert!(repaired.branch_session_id.is_none());
        assert!(repaired.snapshot_consumed_at.is_none());
        assert_eq!(
            pending_snapshot(&db.conn, branch.id)
                .await
                .unwrap()
                .unwrap()
                .context,
            "important context"
        );
        let conversation = conversation_service::get_by_id(&db.conn, branch.id)
            .await
            .unwrap();
        assert_eq!(conversation.external_id, None);
        assert_eq!(conversation.status, "pending_review");
    }

    #[tokio::test]
    async fn merge_is_append_only_and_idempotent() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-branch-merge-test").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let source = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();
        let (branch, _) = create_branch_row(
            &db.conn,
            &source,
            Some("fork-session".into()),
            None,
            "native",
            BranchInheritanceRecord {
                source_session_id: source.external_id.clone(),
                branch_session_id: Some("fork-session".into()),
                inheritance_mode: "native_fork".into(),
                inherited_message_count: 0,
                inherited_context_chars: 0,
                inherited_estimated_tokens: 0,
                inheritance_compressed: false,
                inheritance_truncated: false,
                inheritance_note: None,
                forked_through_at: None,
                snapshot_version: 2,
                snapshot_context: None,
                snapshot_images: Vec::new(),
            },
        )
        .await
        .unwrap();

        let now = Utc::now();
        for (id, path, valid, hidden) in [
            ("selected", "final.pdf", true, false),
            ("not-selected", "notes.txt", true, false),
            ("invalid", "missing.pdf", false, false),
            ("hidden", "internal.log", true, true),
        ] {
            conversation_deliverable::ActiveModel {
                id: Set(id.into()),
                conversation_id: Set(branch.id),
                turn_run_id: Set(None),
                root_path: Set("/tmp/codeg-branch-merge-test".into()),
                path: Set(path.into()),
                kind: Set("file".into()),
                title: Set(path.into()),
                description: Set(None),
                role: Set("primary".into()),
                category: Set("document".into()),
                change_kind: Set("created".into()),
                position: Set(0),
                source: Set("declared".into()),
                file_name: Set(path.into()),
                extension: Set(path.rsplit_once('.').map(|(_, ext)| ext.into())),
                size_bytes: Set(Some(42)),
                modified_at: Set(Some(now)),
                is_valid: Set(valid),
                invalid_reason: Set((!valid).then(|| "missing".into())),
                is_hidden: Set(hidden),
                verified_at: Set(now),
                last_checked_at: Set(Some(now)),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&db.conn)
            .await
            .unwrap();
        }

        let first = merge_branch(
            &db.conn,
            branch.id,
            "merge-once".into(),
            "Branch conclusion".into(),
            vec!["selected".into(), "invalid".into(), "hidden".into()],
        )
        .await
        .unwrap();
        let retry = merge_branch(
            &db.conn,
            branch.id,
            "merge-once".into(),
            "ignored duplicate".into(),
            vec!["selected".into(), "not-selected".into()],
        )
        .await
        .unwrap();
        assert!(!first.deduplicated);
        assert!(retry.deduplicated);
        assert_eq!(first.copied_deliverable_count, 1);
        assert_eq!(retry.copied_deliverable_count, 1);
        let copied = conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(source_id))
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(copied.len(), 1);
        assert_eq!(copied[0].path, "final.pdf");
        assert_eq!(copied[0].source, "branch_merge");
        let pending = pending_merge_context(&db.conn, source_id).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].1.contains("Branch conclusion"));
        assert!(pending[0].1.contains("final.pdf"));
        assert!(!pending[0].1.contains("notes.txt"));
        mark_merge_context_consumed(&db.conn, source_id, &[pending[0].0.clone()])
            .await
            .unwrap();
        assert!(pending_merge_context(&db.conn, source_id)
            .await
            .unwrap()
            .is_empty());
        let turns = merge_turns_for_target(&db.conn, source_id).await.unwrap();
        assert_eq!(turns.len(), 2);
        assert!(matches!(turns[0].role, TurnRole::User));
        assert!(matches!(turns[1].role, TurnRole::Assistant));
        let ContentBlock::Text { text } = &turns[1].blocks[0] else {
            panic!("expected merge summary text")
        };
        assert_eq!(text, "Branch conclusion");

        // Both conversations remain independent live rows.
        assert!(conversation_service::get_by_id(&db.conn, source_id)
            .await
            .is_ok());
        assert!(conversation_service::get_by_id(&db.conn, branch.id)
            .await
            .is_ok());

        conversation_service::soft_delete(&db.conn, branch.id)
            .await
            .unwrap();
        assert!(conversation_service::get_by_id(&db.conn, source_id)
            .await
            .is_ok());
    }
}
