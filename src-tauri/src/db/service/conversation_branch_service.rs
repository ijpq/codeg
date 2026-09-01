use chrono::{Datelike, Local, NaiveDate, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    FromQueryResult, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, Set, TransactionError,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use crate::db::entities::{
    conversation, conversation_branch, conversation_branch_merge, conversation_deliverable,
    conversation_turn_deliverable, conversation_turn_run,
};
use crate::db::error::DbError;
#[cfg(test)]
use crate::db::service::conversation_service;
use crate::db::service::deliverable_path::deliverable_path_identity;
use crate::db::service::deliverable_service;
use crate::models::conversation::DbConversationSummary;
use crate::models::message::{ContentBlock, ImageData, MessageTurn, TurnRole};

static BRANCH_CREATE_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
static BRANCH_MERGE_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

fn source_title_base(title: Option<&str>) -> String {
    let title = title.unwrap_or("未命名会话").trim();
    let title = title.strip_prefix("[Fork]").unwrap_or(title).trim();
    if title.is_empty() {
        "未命名会话".into()
    } else {
        title.into()
    }
}

fn next_dated_branch_title(
    source_title: Option<&str>,
    date: NaiveDate,
    sibling_titles: &std::collections::HashSet<String>,
) -> String {
    let base = format!(
        "{} · 分支 {}.{}",
        source_title_base(source_title),
        date.month(),
        date.day()
    );
    if !sibling_titles.contains(&base) {
        return base;
    }

    (2..)
        .map(|number| format!("{base}.{number}"))
        .find(|candidate| !sibling_titles.contains(candidate))
        .expect("the dated branch title number space is unbounded")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationBranchInfo {
    pub branch_conversation_id: i32,
    pub creation_request_id: Option<String>,
    pub operation_id: Option<String>,
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
    pub source_rollout_offset: Option<i64>,
    pub branch_rollout_offset: Option<i64>,
    pub fork_boundary_kind: Option<String>,
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
            creation_request_id: value.creation_request_id,
            operation_id: value.operation_id,
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
            source_rollout_offset: value.source_rollout_offset,
            branch_rollout_offset: value.branch_rollout_offset,
            fork_boundary_kind: value.fork_boundary_kind,
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
    pub source_rollout_offset: Option<i64>,
    pub branch_rollout_offset: Option<i64>,
    pub fork_boundary_kind: Option<String>,
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
    create_branch_row_with_request(
        conn,
        source,
        None,
        external_id,
        fork_message_id,
        fork_mode,
        inheritance,
    )
    .await
}

pub async fn create_branch_row_with_request(
    conn: &DatabaseConnection,
    source: &DbConversationSummary,
    creation_request_id: Option<String>,
    external_id: Option<String>,
    fork_message_id: Option<String>,
    fork_mode: &str,
    inheritance: BranchInheritanceRecord,
) -> Result<(conversation::Model, ConversationBranchInfo), DbError> {
    let operation_id = creation_request_id.clone();
    create_branch_row_with_operation(
        conn,
        source,
        creation_request_id,
        operation_id,
        external_id,
        fork_message_id,
        fork_mode,
        inheritance,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn create_branch_row_with_operation(
    conn: &DatabaseConnection,
    source: &DbConversationSummary,
    creation_request_id: Option<String>,
    operation_id: Option<String>,
    external_id: Option<String>,
    fork_message_id: Option<String>,
    fork_mode: &str,
    inheritance: BranchInheritanceRecord,
) -> Result<(conversation::Model, ConversationBranchInfo), DbError> {
    let _create_guard = BRANCH_CREATE_LOCK.lock().await;
    if let Some(operation_id) = operation_id.as_deref() {
        if let Some(existing) = conversation_branch::Entity::find()
            .filter(conversation_branch::Column::OperationId.eq(operation_id))
            .one(conn)
            .await?
        {
            let branch = conversation::Entity::find_by_id(existing.branch_conversation_id)
                .one(conn)
                .await?
                .ok_or_else(|| DbError::NotFound("idempotent branch conversation".into()))?;
            return Ok((branch, existing.into()));
        }
    }
    if let Some(request_id) = creation_request_id.as_deref() {
        if let Some(existing) = conversation_branch::Entity::find()
            .filter(conversation_branch::Column::CreationRequestId.eq(request_id))
            .one(conn)
            .await?
        {
            let branch = conversation::Entity::find_by_id(existing.branch_conversation_id)
                .one(conn)
                .await?
                .ok_or_else(|| DbError::NotFound("idempotent branch conversation".into()))?;
            return Ok((branch, existing.into()));
        }
    }
    let sibling_titles = conversation::Entity::find()
        .filter(conversation::Column::FolderId.eq(source.folder_id))
        .filter(conversation::Column::DeletedAt.is_null())
        .all(conn)
        .await?
        .into_iter()
        .filter_map(|row| row.title)
        .collect::<std::collections::HashSet<_>>();
    let title = next_dated_branch_title(
        source.title.as_deref(),
        Local::now().date_naive(),
        &sibling_titles,
    );
    let title = Some(title);
    let now = Utc::now();
    let provisional = fork_mode == "snapshot" && inheritance.snapshot_context.is_some();
    let snapshot_digest = inheritance.snapshot_context.as_deref().map(|context| {
        let mut digest = Sha256::new();
        digest.update(context.as_bytes());
        format!("{:x}", digest.finalize())
    });
    let agent_type = serde_json::to_value(source.agent_type)
        .ok()
        .and_then(|value| value.as_str().map(String::from))
        .unwrap_or_default();
    let folder_id = source.folder_id;
    let source_id = source.id;
    let source_title = source.title.clone();
    let source_kind = source.kind.clone();
    let source_model = source.model.clone();
    let source_git_branch = source.git_branch.clone();
    let source_origin_cwd = source.origin_cwd.clone();
    let fork_mode = fork_mode.to_string();
    let result = conn
        .transaction::<_, (conversation::Model, conversation_branch::Model), sea_orm::DbErr>(
            |txn| {
                Box::pin(async move {
                    // Conversation ownership and its branch relation become
                    // visible atomically. A cancelled HTTP request or process
                    // error can therefore never expose a title-only branch.
                    let created = conversation::ActiveModel {
                        id: NotSet,
                        folder_id: Set(folder_id),
                        title: Set(title),
                        title_locked: Set(true),
                        agent_type: Set(agent_type),
                        status: Set(conversation::ConversationStatus::PendingReview),
                        kind: Set(source_kind),
                        model: Set(source_model),
                        git_branch: Set(source_git_branch),
                        external_id: Set(external_id),
                        parent_id: Set(None),
                        parent_tool_use_id: Set(None),
                        delegation_call_id: Set(None),
                        message_count: Set(0),
                        created_at: Set(now),
                        updated_at: Set(now),
                        deleted_at: Set(None),
                        pinned_at: Set(None),
                        origin_cwd: Set(source_origin_cwd),
                    }
                    .insert(txn)
                    .await?;
                    let relation = conversation_branch::ActiveModel {
                        branch_conversation_id: Set(created.id),
                        creation_request_id: Set(creation_request_id),
                        operation_id: Set(operation_id),
                        source_conversation_id: Set(source_id),
                        source_title: Set(source_title),
                        fork_message_id: Set(fork_message_id),
                        fork_mode: Set(fork_mode),
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
                        source_rollout_offset: Set(inheritance.source_rollout_offset),
                        branch_rollout_offset: Set(inheritance.branch_rollout_offset),
                        fork_boundary_kind: Set(inheritance.fork_boundary_kind),
                        snapshot_version: Set(inheritance.snapshot_version),
                        snapshot_images_json: Set((!inheritance.snapshot_images.is_empty()).then(
                            || {
                                serde_json::to_string(&inheritance.snapshot_images)
                                    .unwrap_or_else(|_| "[]".into())
                            },
                        )),
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
                    .insert(txn)
                    .await?;
                    Ok((created, relation))
                })
            },
        )
        .await;
    match result {
        Ok((created, relation)) => Ok((created, relation.into())),
        Err(TransactionError::Connection(error) | TransactionError::Transaction(error)) => {
            Err(DbError::Database(error))
        }
    }
}

pub async fn get_by_operation_id(
    conn: &DatabaseConnection,
    operation_id: &str,
) -> Result<Option<ConversationBranchInfo>, DbError> {
    let operation_id = operation_id.trim();
    if operation_id.is_empty() {
        return Ok(None);
    }
    Ok(conversation_branch::Entity::find()
        .filter(conversation_branch::Column::OperationId.eq(operation_id))
        .one(conn)
        .await?
        .map(Into::into))
}

pub async fn branch_conversation_is_deleted(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<bool, DbError> {
    Ok(conversation::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
        .is_none_or(|conversation| conversation.deleted_at.is_some()))
}

pub async fn get_by_creation_request_id(
    conn: &DatabaseConnection,
    request_id: &str,
) -> Result<Option<ConversationBranchInfo>, DbError> {
    let request_id = request_id.trim();
    if request_id.is_empty() {
        return Ok(None);
    }
    Ok(conversation_branch::Entity::find()
        .filter(conversation_branch::Column::CreationRequestId.eq(request_id))
        .one(conn)
        .await?
        .map(Into::into))
}

/// Lightweight source-branch metadata.  Ordinary source-conversation opens do
/// not call this query, and an explicit branch-list request never selects the
/// potentially large snapshot context or any branch rollout content.
#[derive(Debug, Clone, FromQueryResult, Serialize)]
pub struct ConversationSourceBranchPreview {
    pub branch_conversation_id: i32,
    pub branch_session_id: Option<String>,
    pub fork_mode: String,
    pub inheritance_mode: String,
    pub inherited_message_count: i32,
    pub inherited_context_chars: i64,
    pub inherited_estimated_tokens: i64,
    pub lifecycle_state: String,
    pub created_at: chrono::DateTime<Utc>,
    pub last_merged_at: Option<chrono::DateTime<Utc>>,
    pub merge_target_conversation_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationSourceBranchPreviewPage {
    pub items: Vec<ConversationSourceBranchPreview>,
    pub next_offset: Option<u64>,
    pub has_more: bool,
}

pub async fn branch_preview_page_for_source(
    conn: &DatabaseConnection,
    conversation_id: i32,
    offset: u64,
    limit: u64,
) -> Result<ConversationSourceBranchPreviewPage, DbError> {
    let limit = limit.clamp(1, 50);
    let mut rows = conversation_branch::Entity::find()
        .select_only()
        .column(conversation_branch::Column::BranchConversationId)
        .column(conversation_branch::Column::BranchSessionId)
        .column(conversation_branch::Column::ForkMode)
        .column(conversation_branch::Column::InheritanceMode)
        .column(conversation_branch::Column::InheritedMessageCount)
        .column(conversation_branch::Column::InheritedContextChars)
        .column(conversation_branch::Column::InheritedEstimatedTokens)
        .column(conversation_branch::Column::LifecycleState)
        .column(conversation_branch::Column::CreatedAt)
        .column(conversation_branch::Column::LastMergedAt)
        .column(conversation_branch::Column::MergeTargetConversationId)
        .filter(conversation_branch::Column::SourceConversationId.eq(conversation_id))
        .order_by_desc(conversation_branch::Column::CreatedAt)
        .offset(offset)
        .limit(limit + 1)
        .into_model::<ConversationSourceBranchPreview>()
        .all(conn)
        .await?;
    let has_more = rows.len() as u64 > limit;
    rows.truncate(limit as usize);
    Ok(ConversationSourceBranchPreviewPage {
        next_offset: has_more.then_some(offset + rows.len() as u64),
        has_more,
        items: rows,
    })
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
    "pending_first_prompt",
    "first_prompt_queued",
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
    let already_safe = branch.branch_session_id.is_none()
        && conversation.external_id.is_none()
        && conversation.status == conversation::ConversationStatus::PendingReview
        && PROVISIONAL_STATES.contains(&branch.lifecycle_state.as_str());
    if already_safe
        && matches!(
            branch.lifecycle_state.as_str(),
            "provisional" | "pending_first_prompt" | "first_prompt_queued" | "retryable_failed"
        )
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

pub async fn is_merged_branch(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<bool, DbError> {
    Ok(conversation_branch::Entity::find_by_id(conversation_id)
        .filter(conversation_branch::Column::LifecycleState.eq("merged"))
        .one(conn)
        .await?
        .is_some())
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
    // `prompt_ready` is reserved for a branch with a verified durable session.
    // A provisional session/new connection can accept its first prompt, but it
    // has no resumable rollout yet and must remain pending-first-prompt.
    let lifecycle_state = if lifecycle_state == "prompt_ready"
        && (row.branch_session_id.is_none() || row.session_verified_at.is_none())
    {
        "pending_first_prompt"
    } else {
        lifecycle_state
    };
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
    active.lifecycle_state = Set("first_prompt_queued".into());
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

/// Native branches that may have been created by the legacy in-process fork
/// path. Older codex-acp versions kept both the parent and child writer inside
/// that one adapter process; if restoring the parent reports `active writer`,
/// an idle child connection from this list is the only CodeG-owned process that
/// can safely release it. The caller still verifies connection state before
/// disconnecting anything.
pub async fn native_branch_sessions_for_source(
    conn: &DatabaseConnection,
    source_session_id: &str,
) -> Result<Vec<(i32, String)>, DbError> {
    Ok(conversation_branch::Entity::find()
        .filter(conversation_branch::Column::ForkMode.eq("native"))
        .filter(conversation_branch::Column::SourceSessionId.eq(source_session_id))
        // New detached-writer branches always persist a creation id. Only
        // pre-upgrade rows used the adapter process that retained the parent
        // writer, so never tear down a healthy modern branch during repair.
        .filter(conversation_branch::Column::CreationRequestId.is_null())
        .all(conn)
        .await?
        .into_iter()
        .filter_map(|row| {
            row.branch_session_id
                .filter(|session_id| !session_id.trim().is_empty())
                .map(|session_id| (row.branch_conversation_id, session_id))
        })
        .collect())
}

/// Normalize user-visible metadata left by the legacy two-row swap fork. This
/// never changes either conversation's external session id or source/branch
/// mapping; when ownership is ambiguous it deliberately leaves the data alone.
pub async fn normalize_legacy_branch_metadata(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<bool, DbError> {
    let _guard = BRANCH_CREATE_LOCK.lock().await;
    let Some(relation) = conversation_branch::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    let Some(branch) = conversation::Entity::find_by_id(conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    let legacy_title = branch
        .title
        .as_deref()
        .is_some_and(|title| title.trim_start().starts_with("[Fork]"));
    if !legacy_title {
        return Ok(false);
    }
    let Some(source) = conversation::Entity::find_by_id(relation.source_conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    let base = format!("{} · 分支", source_title_base(source.title.as_deref()));
    let used = conversation::Entity::find()
        .filter(conversation::Column::FolderId.eq(branch.folder_id))
        .filter(conversation::Column::DeletedAt.is_null())
        .all(conn)
        .await?
        .into_iter()
        .filter(|row| row.id != conversation_id)
        .filter_map(|row| row.title)
        .collect::<std::collections::HashSet<_>>();
    let title = if !used.contains(&base) {
        base
    } else {
        (2..)
            .map(|number| format!("{base} {number}"))
            .find(|candidate| !used.contains(candidate))
            .expect("the branch title number space is unbounded")
    };
    let mut active = branch.into_active_model();
    active.title = Set(Some(title.clone()));
    active.title_locked = Set(true);
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    tracing::info!(
        branch_conversation_id = conversation_id,
        source_conversation_id = relation.source_conversation_id,
        normalized_title = title,
        repair = true,
        "[ACP][branch] normalized legacy fork title without changing session ownership"
    );
    Ok(true)
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
        creation_request_id: Set(None),
        operation_id: Set(None),
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
        // The old fork implementation did not persist an explicit message
        // boundary. The branch row's creation timestamp is the narrowest
        // durable boundary available; using the source creation time hid
        // virtually the entire inherited history, while using its current
        // message count could leak source turns written after the fork.
        forked_through_at: Set(Some(branch.created_at)),
        source_rollout_offset: Set(None),
        branch_rollout_offset: Set(None),
        fork_boundary_kind: Set(None),
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

/// Fast idempotency probe used before transcript extraction. Without this, a
/// retried click on a successfully merged multi-gigabyte native branch would
/// repeat the expensive read before `merge_branch` could discover the audit
/// row inside its transaction lock.
pub async fn existing_merge_result(
    conn: &DatabaseConnection,
    branch_conversation_id: i32,
    request_id: &str,
) -> Result<Option<MergeBranchResult>, DbError> {
    if let Some(existing) = conversation_branch_merge::Entity::find_by_id(request_id.trim())
        .one(conn)
        .await?
    {
        let copied = serde_json::from_str::<Vec<String>>(&existing.deliverable_ids_json)
            .unwrap_or_default()
            .len();
        return Ok(Some(MergeBranchResult {
            merge_id: existing.id,
            target_conversation_id: existing.target_conversation_id,
            copied_deliverable_count: copied,
            deduplicated: true,
        }));
    }
    let Some(relation) = conversation_branch::Entity::find_by_id(branch_conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(None);
    };
    let Some(last_merge_key) = relation
        .last_merge_key
        .as_deref()
        .filter(|_| relation.lifecycle_state == "merged")
    else {
        return Ok(None);
    };
    let Some(existing) = conversation_branch_merge::Entity::find_by_id(last_merge_key)
        .one(conn)
        .await?
    else {
        return Ok(None);
    };
    let copied = serde_json::from_str::<Vec<String>>(&existing.deliverable_ids_json)
        .unwrap_or_default()
        .len();
    Ok(Some(MergeBranchResult {
        merge_id: existing.id,
        target_conversation_id: existing.target_conversation_id,
        copied_deliverable_count: copied,
        deduplicated: true,
    }))
}

pub async fn record_inferred_branch_boundary(
    conn: &DatabaseConnection,
    branch_conversation_id: i32,
    branch_rollout_offset: i64,
) -> Result<ConversationBranchInfo, DbError> {
    let row = conversation_branch::Entity::find_by_id(branch_conversation_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound("conversation branch".into()))?;
    if row.branch_rollout_offset.is_some() {
        return Ok(row.into());
    }
    let mut active = row.into_active_model();
    active.branch_rollout_offset = Set(Some(branch_rollout_offset));
    active.fork_boundary_kind = Set(Some("inferred_bounded_tail".into()));
    active.lifecycle_updated_at = Set(Some(Utc::now()));
    Ok(active.update(conn).await?.into())
}

fn merge_source_priority(source: &str) -> u8 {
    match source {
        deliverable_service::SOURCE_DECLARED => 3,
        "branch_merge" => 2,
        deliverable_service::SOURCE_INFERRED => 1,
        _ => 0,
    }
}

fn branch_deliverable_is_mergeable(item: &conversation_deliverable::Model) -> bool {
    if !item.is_valid || item.is_hidden || item.change_kind == "deleted" {
        return false;
    }
    item.source != deliverable_service::SOURCE_INFERRED
        || (item.category == "standalone_output"
            && deliverable_service::inference_path_allowed(&item.path))
}

fn select_merge_deliverables(
    rows: Vec<conversation_deliverable::Model>,
) -> Vec<conversation_deliverable::Model> {
    let mut selected: HashMap<String, conversation_deliverable::Model> = HashMap::new();
    for row in rows.into_iter().filter(branch_deliverable_is_mergeable) {
        let identity = deliverable_path_identity(&row.root_path, &row.path).identity;
        let should_replace = selected.get(&identity).is_none_or(|current| {
            merge_source_priority(&row.source)
                .cmp(&merge_source_priority(&current.source))
                .then_with(|| row.updated_at.cmp(&current.updated_at))
                .then_with(|| row.created_at.cmp(&current.created_at))
                .is_gt()
        });
        if should_replace {
            selected.insert(identity, row);
        }
    }
    let mut selected = selected.into_values().collect::<Vec<_>>();
    selected.sort_by(|left, right| {
        left.position
            .cmp(&right.position)
            .then_with(|| left.created_at.cmp(&right.created_at))
            .then_with(|| left.path.cmp(&right.path))
    });
    selected
}

fn set_merge_transaction_stage(stage: &Arc<Mutex<&'static str>>, next: &'static str) {
    if let Ok(mut current) = stage.lock() {
        *current = next;
    }
}

fn sqlite_failure_kind(error: &sea_orm::DbErr) -> &'static str {
    let text = error.to_string().to_ascii_lowercase();
    if text.contains("unique constraint") {
        "unique_constraint"
    } else if text.contains("foreign key constraint") {
        "foreign_key_constraint"
    } else if text.contains("database is locked") || text.contains("database is busy") {
        "database_busy"
    } else {
        "database_error"
    }
}

fn sqlite_constraint_name(error: &sea_orm::DbErr) -> &'static str {
    let text = error.to_string();
    if text.contains("conversation_deliverable.conversation_id")
        && text.contains("conversation_deliverable.root_path")
        && text.contains("conversation_deliverable.path")
    {
        "idx_conversation_deliverable_conversation_path"
    } else if text.contains("conversation_branch_merge.id") {
        "conversation_branch_merge_primary_key"
    } else {
        "unknown"
    }
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
    // The desktop/server runtime owns one SQLite database in one process.
    // Serialize the read → append → lifecycle transition so two browser tabs
    // using different request ids still produce exactly one merge event. The
    // stable request id remains the cross-retry idempotency key.
    let _merge_guard = BRANCH_MERGE_LOCK.lock().await;
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
    if relation.lifecycle_state == "merged" {
        if let Some(existing_id) = relation.last_merge_key.as_deref() {
            if let Some(existing) = conversation_branch_merge::Entity::find_by_id(existing_id)
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
        }
        return Err(DbError::Validation(
            "This branch has already been returned to its source conversation".into(),
        ));
    }
    // Deletion is soft and relations intentionally do not cascade. Reject a
    // merge when the source is gone; deleting either side never deletes the
    // other side or the audit row.
    conversation::Entity::find_by_id(relation.source_conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound("source conversation".into()))?;

    let mut deliverable_query = conversation_deliverable::Entity::find()
        .filter(conversation_deliverable::Column::ConversationId.eq(branch_conversation_id))
        .filter(conversation_deliverable::Column::IsValid.eq(true))
        .filter(conversation_deliverable::Column::IsHidden.eq(false));
    if !deliverable_ids.is_empty() {
        deliverable_query = deliverable_query
            .filter(conversation_deliverable::Column::Id.is_in(deliverable_ids.clone()));
    }
    let selected = select_merge_deliverables(
        deliverable_query
            .order_by_desc(conversation_deliverable::Column::UpdatedAt)
            .all(conn)
            .await?,
    );
    let copied_count = selected.len();
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
    let transaction_stage = Arc::new(Mutex::new("begin"));
    let transaction_stage_for_tx = transaction_stage.clone();
    let transaction_result = conn
        .transaction::<_, (), sea_orm::DbErr>(|txn| {
            Box::pin(async move {
                set_merge_transaction_stage(&transaction_stage_for_tx, "insert_turn_run");
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

                set_merge_transaction_stage(
                    &transaction_stage_for_tx,
                    "load_target_deliverables",
                );
                let target_rows = conversation_deliverable::Entity::find()
                    .filter(conversation_deliverable::Column::ConversationId.eq(target_id))
                    .order_by_desc(conversation_deliverable::Column::UpdatedAt)
                    .all(txn)
                    .await?;
                let mut target_by_identity = HashMap::new();
                let mut legacy_duplicate_count = 0usize;
                for target in target_rows {
                    let identity =
                        deliverable_path_identity(&target.root_path, &target.path).identity;
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        target_by_identity.entry(identity)
                    {
                        entry.insert(target);
                    } else {
                        legacy_duplicate_count += 1;
                    }
                }
                if legacy_duplicate_count > 0 {
                    tracing::warn!(
                        branch_conversation_id,
                        target_conversation_id = target_id,
                        merge_request_id = %request_for_tx,
                        legacy_duplicate_count,
                        stage = "deliverable_identity_lookup",
                        "[ACP][branch] legacy deliverable path aliases retained; newest identity reused"
                    );
                }

                let mut target_deliverable_ids = Vec::with_capacity(selected.len());
                for (position, source) in selected.into_iter().enumerate() {
                    set_merge_transaction_stage(
                        &transaction_stage_for_tx,
                        "upsert_target_deliverable",
                    );
                    let normalized = deliverable_path_identity(&source.root_path, &source.path);
                    let target = if let Some(existing) =
                        target_by_identity.get(&normalized.identity).cloned()
                    {
                        // `conversation_deliverable` is the durable file
                        // identity. Updating it records the newest aggregate
                        // metadata; the association inserted below preserves a
                        // separate, auditable version for this merge turn.
                        let mut active = existing.into_active_model();
                        active.turn_run_id = Set(Some(run_id.clone()));
                        active.kind = Set(source.kind.clone());
                        active.title = Set(source.title.clone());
                        active.description = Set(source.description.clone());
                        active.role = Set(source.role.clone());
                        active.category = Set(source.category.clone());
                        active.change_kind = Set(source.change_kind.clone());
                        active.position = Set(position as i32);
                        active.source = Set("branch_merge".into());
                        active.file_name = Set(source.file_name.clone());
                        active.extension = Set(source.extension.clone());
                        active.size_bytes = Set(source.size_bytes);
                        active.modified_at = Set(source.modified_at);
                        active.is_valid = Set(true);
                        active.invalid_reason = Set(None);
                        active.is_hidden = Set(false);
                        active.verified_at = Set(source.verified_at);
                        active.last_checked_at = Set(source.last_checked_at);
                        active.updated_at = Set(now);
                        active.update(txn).await?
                    } else {
                        conversation_deliverable::ActiveModel {
                            id: Set(uuid::Uuid::new_v4().to_string()),
                            conversation_id: Set(target_id),
                            turn_run_id: Set(Some(run_id.clone())),
                            root_path: Set(normalized.storage_root),
                            path: Set(normalized.storage_path),
                            kind: Set(source.kind.clone()),
                            title: Set(source.title.clone()),
                            description: Set(source.description.clone()),
                            role: Set(source.role.clone()),
                            category: Set(source.category.clone()),
                            change_kind: Set(source.change_kind.clone()),
                            position: Set(position as i32),
                            source: Set("branch_merge".into()),
                            file_name: Set(source.file_name.clone()),
                            extension: Set(source.extension.clone()),
                            size_bytes: Set(source.size_bytes),
                            modified_at: Set(source.modified_at),
                            is_valid: Set(true),
                            invalid_reason: Set(None),
                            is_hidden: Set(false),
                            verified_at: Set(source.verified_at),
                            last_checked_at: Set(source.last_checked_at),
                            created_at: Set(now),
                            updated_at: Set(now),
                        }
                        .insert(txn)
                        .await?
                    };
                    let target_id_for_file = target.id.clone();
                    target_by_identity.insert(normalized.identity, target);
                    target_deliverable_ids.push(target_id_for_file.clone());

                    set_merge_transaction_stage(
                        &transaction_stage_for_tx,
                        "insert_turn_deliverable",
                    );
                    conversation_turn_deliverable::ActiveModel {
                        id: Set(uuid::Uuid::new_v4().to_string()),
                        conversation_id: Set(target_id),
                        turn_run_id: Set(run_id.clone()),
                        deliverable_id: Set(target_id_for_file),
                        source: Set("branch_merge".into()),
                        title: Set(source.title),
                        description: Set(source.description),
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

                // New rows store target-conversation deliverable identities.
                // Older audit rows may contain branch ids; readers continue to
                // tolerate that historical representation.
                set_merge_transaction_stage(&transaction_stage_for_tx, "insert_merge_audit");
                conversation_branch_merge::ActiveModel {
                    id: Set(request_for_tx.clone()),
                    branch_conversation_id: Set(branch_conversation_id),
                    source_conversation_id: Set(target_id),
                    target_conversation_id: Set(target_id),
                    summary: Set(summary_for_tx),
                    deliverable_ids_json: Set(
                        serde_json::to_string(&target_deliverable_ids)
                            .unwrap_or_else(|_| "[]".into()),
                    ),
                    created_at: Set(now),
                    context_consumed_at: Set(None),
                }
                .insert(txn)
                .await?;

                set_merge_transaction_stage(&transaction_stage_for_tx, "update_branch_state");
                let mut active = relation.into_active_model();
                active.last_merged_at = Set(Some(now));
                active.last_merge_key = Set(Some(request_for_tx));
                active.merge_target_conversation_id = Set(Some(target_id));
                active.lifecycle_state = Set("merged".into());
                active.lifecycle_error = Set(None);
                active.lifecycle_updated_at = Set(Some(now));
                active.update(txn).await?;

                set_merge_transaction_stage(
                    &transaction_stage_for_tx,
                    "update_branch_conversation",
                );
                if let Some(branch_conversation) =
                    conversation::Entity::find_by_id(branch_conversation_id)
                        .one(txn)
                        .await?
                {
                    let mut branch_active = branch_conversation.into_active_model();
                    branch_active.status = Set(conversation::ConversationStatus::Completed);
                    branch_active.pinned_at = Set(None);
                    branch_active.updated_at = Set(now);
                    branch_active.update(txn).await?;
                }
                set_merge_transaction_stage(
                    &transaction_stage_for_tx,
                    "update_target_conversation",
                );
                if let Some(target) = conversation::Entity::find_by_id(target_id).one(txn).await? {
                    let next_count = target.message_count.saturating_add(2);
                    let mut target_active = target.into_active_model();
                    target_active.message_count = Set(next_count);
                    target_active.updated_at = Set(now);
                    target_active.update(txn).await?;
                }
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
        let database_error = match error {
            TransactionError::Connection(error) | TransactionError::Transaction(error) => error,
        };
        let failure_stage = transaction_stage
            .lock()
            .map(|stage| *stage)
            .unwrap_or("unknown");
        tracing::error!(
            branch_conversation_id,
            target_conversation_id = target_id,
            merge_request_id = %request_id,
            failure_stage,
            sqlite_error_kind = sqlite_failure_kind(&database_error),
            sqlite_constraint_name = sqlite_constraint_name(&database_error),
            database_error = %database_error,
            database_error_debug = ?database_error,
            transaction_committed = false,
            "[ACP][branch] merge database transaction rolled back"
        );
        return Err(DbError::Database(database_error));
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

/// Lightweight branch-return metadata for the conversation timeline.  The
/// potentially large summary body is deliberately projected to a short SQL
/// preview so SQLite never materializes every historical return in an ordinary
/// conversation open.
#[derive(Debug, Clone, FromQueryResult, Serialize)]
pub struct ConversationBranchMergePreview {
    pub id: String,
    pub branch_conversation_id: i32,
    pub created_at: chrono::DateTime<Utc>,
    pub summary_preview: String,
    pub summary_bytes: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationBranchMergePreviewPage {
    pub items: Vec<ConversationBranchMergePreview>,
    pub next_offset: Option<u64>,
    pub has_more: bool,
}

pub async fn merge_preview_page_for_target(
    conn: &DatabaseConnection,
    conversation_id: i32,
    offset: u64,
    limit: u64,
) -> Result<ConversationBranchMergePreviewPage, DbError> {
    let limit = limit.clamp(1, 50);
    let mut rows = conversation_branch_merge::Entity::find()
        .select_only()
        .column(conversation_branch_merge::Column::Id)
        .column(conversation_branch_merge::Column::BranchConversationId)
        .column(conversation_branch_merge::Column::CreatedAt)
        .column_as(Expr::cust("substr(summary, 1, 512)"), "summary_preview")
        .column_as(Expr::cust("length(CAST(summary AS BLOB))"), "summary_bytes")
        .filter(conversation_branch_merge::Column::TargetConversationId.eq(conversation_id))
        .order_by_desc(conversation_branch_merge::Column::CreatedAt)
        .offset(offset)
        .limit(limit + 1)
        .into_model::<ConversationBranchMergePreview>()
        .all(conn)
        .await?;
    let has_more = rows.len() as u64 > limit;
    rows.truncate(limit as usize);
    Ok(ConversationBranchMergePreviewPage {
        next_offset: has_more.then_some(offset + rows.len() as u64),
        has_more,
        items: rows,
    })
}

pub async fn merge_previews_for_target(
    conn: &DatabaseConnection,
    conversation_id: i32,
    limit: u64,
) -> Result<Vec<ConversationBranchMergePreview>, DbError> {
    let mut rows = merge_preview_page_for_target(conn, conversation_id, 0, limit)
        .await?
        .items;
    rows.reverse();
    Ok(rows)
}

pub async fn merge_summary_for_target(
    conn: &DatabaseConnection,
    conversation_id: i32,
    merge_id: &str,
) -> Result<Option<String>, DbError> {
    Ok(conversation_branch_merge::Entity::find_by_id(merge_id)
        .filter(conversation_branch_merge::Column::TargetConversationId.eq(conversation_id))
        .one(conn)
        .await?
        .map(|row| row.summary))
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
    use sea_orm::{ConnectionTrait, DatabaseBackend, PaginatorTrait, Statement};

    struct TestDeliverable<'a> {
        id: &'a str,
        conversation_id: i32,
        root_path: &'a str,
        path: &'a str,
        source: &'a str,
        valid: bool,
        hidden: bool,
        change_kind: &'a str,
        category: &'a str,
    }

    async fn seed_test_deliverable(conn: &DatabaseConnection, input: TestDeliverable<'_>) {
        let TestDeliverable {
            id,
            conversation_id,
            root_path,
            path,
            source,
            valid,
            hidden,
            change_kind,
            category,
        } = input;
        let now = Utc::now();
        conversation_deliverable::ActiveModel {
            id: Set(id.into()),
            conversation_id: Set(conversation_id),
            turn_run_id: Set(None),
            root_path: Set(root_path.into()),
            path: Set(path.into()),
            kind: Set("file".into()),
            title: Set(format!("title-{id}")),
            description: Set(Some(format!("description-{id}"))),
            role: Set("supporting".into()),
            category: Set(category.into()),
            change_kind: Set(change_kind.into()),
            position: Set(0),
            source: Set(source.into()),
            file_name: Set(path.rsplit(['/', '\\']).next().unwrap_or(path).into()),
            extension: Set(path.rsplit_once('.').map(|(_, extension)| extension.into())),
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
        .insert(conn)
        .await
        .unwrap();
    }

    async fn seed_native_test_branch(
        db: &crate::db::AppDatabase,
        source_id: i32,
    ) -> conversation::Model {
        let source = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();
        create_branch_row(
            &db.conn,
            &source,
            Some(format!("fork-session-{source_id}")),
            None,
            "native",
            BranchInheritanceRecord {
                source_session_id: source.external_id.clone(),
                branch_session_id: Some(format!("fork-session-{source_id}")),
                inheritance_mode: "native_fork".into(),
                inherited_message_count: 0,
                inherited_context_chars: 0,
                inherited_estimated_tokens: 0,
                inheritance_compressed: false,
                inheritance_truncated: false,
                inheritance_note: None,
                forked_through_at: None,
                source_rollout_offset: None,
                branch_rollout_offset: None,
                fork_boundary_kind: None,
                snapshot_version: 2,
                snapshot_context: None,
                snapshot_images: Vec::new(),
            },
        )
        .await
        .unwrap()
        .0
    }

    #[test]
    fn dated_branch_titles_use_dot_suffixes_for_same_day_siblings() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 25).unwrap();
        let mut used = std::collections::HashSet::new();

        let first = next_dated_branch_title(Some("Agent 调试"), date, &used);
        assert_eq!(first, "Agent 调试 · 分支 8.25");
        used.insert(first);

        let second = next_dated_branch_title(Some("Agent 调试"), date, &used);
        assert_eq!(second, "Agent 调试 · 分支 8.25.2");
        used.insert(second);

        assert_eq!(
            next_dated_branch_title(Some("Agent 调试"), date, &used),
            "Agent 调试 · 分支 8.25.3"
        );
    }

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
        let branch_created_at = conversation::Entity::find_by_id(branch_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap()
            .created_at;
        assert_eq!(
            repaired.forked_through_at,
            Some(branch_created_at),
            "legacy native branches freeze visible history at their own creation time"
        );
        normalize_legacy_branch_metadata(&db.conn, branch_id)
            .await
            .unwrap();
        assert_eq!(
            conversation_service::get_by_id(&db.conn, branch_id)
                .await
                .unwrap()
                .title
                .as_deref(),
            Some("未命名会话 · 分支")
        );
        assert_eq!(
            repair_native_fork_relation(&db.conn, branch_id, "session-parent")
                .await
                .unwrap()
                .unwrap()
                .source_conversation_id,
            source_id,
            "repair is idempotent"
        );
        assert_eq!(
            native_branch_sessions_for_source(&db.conn, "session-parent")
                .await
                .unwrap(),
            vec![(branch_id, "session-child".into())],
            "only legacy native branches are candidates for writer handoff"
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
                source_rollout_offset: None,
                branch_rollout_offset: None,
                fork_boundary_kind: None,
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
        mark_initialization_state(
            &db.conn,
            branch.id,
            "prompt_ready",
            Some("ephemeral-connection".into()),
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
            "pending_first_prompt",
            "an ephemeral session cannot advertise durable prompt_ready"
        );
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
            "first_prompt_queued",
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
    async fn branch_creation_is_idempotent_and_numbers_dated_titles() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-branch-title-test").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        conversation_service::update_title(&db.conn, source_id, "Agent 调试".into())
            .await
            .unwrap();
        let source = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();
        let inheritance = || BranchInheritanceRecord {
            source_session_id: None,
            branch_session_id: None,
            inheritance_mode: "structured_snapshot".into(),
            inherited_message_count: 0,
            inherited_context_chars: 7,
            inherited_estimated_tokens: 2,
            inheritance_compressed: false,
            inheritance_truncated: false,
            inheritance_note: None,
            forked_through_at: None,
            source_rollout_offset: None,
            branch_rollout_offset: None,
            fork_boundary_kind: None,
            snapshot_version: 2,
            snapshot_context: Some("context".into()),
            snapshot_images: Vec::new(),
        };

        let (first, first_info) = create_branch_row_with_request(
            &db.conn,
            &source,
            Some("create-once".into()),
            None,
            None,
            "snapshot",
            inheritance(),
        )
        .await
        .unwrap();
        let (retry, retry_info) = create_branch_row_with_request(
            &db.conn,
            &source,
            Some("create-once".into()),
            None,
            None,
            "snapshot",
            inheritance(),
        )
        .await
        .unwrap();
        let (transport_retry, transport_retry_info) = create_branch_row_with_operation(
            &db.conn,
            &source,
            Some("different-http-request".into()),
            Some("create-once".into()),
            None,
            None,
            "snapshot",
            inheritance(),
        )
        .await
        .unwrap();
        let (second, _) = create_branch_row_with_request(
            &db.conn,
            &source,
            Some("create-twice".into()),
            None,
            None,
            "snapshot",
            inheritance(),
        )
        .await
        .unwrap();

        assert_eq!(first.id, retry.id);
        assert_eq!(first.id, transport_retry.id);
        assert_eq!(
            first_info.branch_conversation_id,
            retry_info.branch_conversation_id
        );
        assert_eq!(
            first_info.branch_conversation_id,
            transport_retry_info.branch_conversation_id
        );
        let first_title = first.title.as_deref().unwrap();
        assert!(first_title.starts_with("Agent 调试 · 分支 "));
        let expected_second_title = format!("{first_title}.2");
        assert_eq!(
            second.title.as_deref(),
            Some(expected_second_title.as_str())
        );
        assert_eq!(
            conversation_branch::Entity::find()
                .count(&db.conn)
                .await
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn native_branch_persistence_keeps_source_session_and_title_immutable() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-native-branch-map").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        conversation_service::bind_external_id(&db.conn, source_id, "source-session", &[])
            .await
            .unwrap();
        conversation_service::update_title(&db.conn, source_id, "Agent 调试".into())
            .await
            .unwrap();
        let source_before = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();

        let (branch, relation) = create_branch_row_with_request(
            &db.conn,
            &source_before,
            Some("native-create-once".into()),
            Some("branch-session".into()),
            None,
            "native",
            BranchInheritanceRecord {
                source_session_id: Some("source-session".into()),
                branch_session_id: Some("branch-session".into()),
                inheritance_mode: "native_fork".into(),
                inherited_message_count: 12,
                inherited_context_chars: 0,
                inherited_estimated_tokens: 0,
                inheritance_compressed: false,
                inheritance_truncated: false,
                inheritance_note: None,
                forked_through_at: Some(Utc::now()),
                source_rollout_offset: Some(123),
                branch_rollout_offset: Some(456),
                fork_boundary_kind: Some("exact_rollout_offset".into()),
                snapshot_version: 2,
                snapshot_context: None,
                snapshot_images: Vec::new(),
            },
        )
        .await
        .unwrap();

        let source_after = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();
        assert_ne!(branch.id, source_id);
        assert_eq!(source_after.title, source_before.title);
        assert_eq!(source_after.external_id.as_deref(), Some("source-session"));
        assert!(branch
            .title
            .as_deref()
            .is_some_and(|title| title.starts_with("Agent 调试 · 分支 ")));
        assert_eq!(branch.external_id.as_deref(), Some("branch-session"));
        assert_eq!(relation.source_conversation_id, source_id);
        assert_eq!(
            relation.branch_session_id.as_deref(),
            Some("branch-session")
        );
        assert_eq!(relation.source_rollout_offset, Some(123));
        assert_eq!(relation.branch_rollout_offset, Some(456));
        assert_eq!(
            relation.fork_boundary_kind.as_deref(),
            Some("exact_rollout_offset")
        );
        assert!(
            native_branch_sessions_for_source(&db.conn, "source-session")
                .await
                .unwrap()
                .is_empty(),
            "detached-writer branches must never be treated as legacy owners"
        );
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
                source_rollout_offset: None,
                branch_rollout_offset: None,
                fork_boundary_kind: None,
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
    async fn legacy_native_boundary_is_persisted_once_without_overwriting_exact_data() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-native-boundary-repair").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let source = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();
        let (branch, _) = create_branch_row(
            &db.conn,
            &source,
            Some("child-session".into()),
            None,
            "native",
            BranchInheritanceRecord {
                source_session_id: Some("source-session".into()),
                branch_session_id: Some("child-session".into()),
                inheritance_mode: "native_fork".into(),
                inherited_message_count: 10,
                inherited_context_chars: 0,
                inherited_estimated_tokens: 0,
                inheritance_compressed: false,
                inheritance_truncated: false,
                inheritance_note: None,
                forked_through_at: Some(Utc::now()),
                source_rollout_offset: None,
                branch_rollout_offset: None,
                fork_boundary_kind: None,
                snapshot_version: 2,
                snapshot_context: None,
                snapshot_images: Vec::new(),
            },
        )
        .await
        .unwrap();

        let inferred = record_inferred_branch_boundary(&db.conn, branch.id, 900)
            .await
            .unwrap();
        assert_eq!(inferred.branch_rollout_offset, Some(900));
        assert_eq!(
            inferred.fork_boundary_kind.as_deref(),
            Some("inferred_bounded_tail")
        );
        let retry = record_inferred_branch_boundary(&db.conn, branch.id, 1000)
            .await
            .unwrap();
        assert_eq!(retry.branch_rollout_offset, Some(900));
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
                source_rollout_offset: None,
                branch_rollout_offset: None,
                fork_boundary_kind: None,
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
            Vec::new(),
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
        let second_tab_retry = merge_branch(
            &db.conn,
            branch.id,
            "merge-from-another-tab".into(),
            "must not be appended twice".into(),
            Vec::new(),
        )
        .await
        .unwrap();
        assert!(!first.deduplicated);
        assert!(retry.deduplicated);
        assert!(second_tab_retry.deduplicated);
        assert_eq!(second_tab_retry.merge_id, first.merge_id);
        assert_eq!(first.copied_deliverable_count, 2);
        assert_eq!(retry.copied_deliverable_count, 2);
        let copied = conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(source_id))
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(copied.len(), 2);
        assert_eq!(copied[0].path, "final.pdf");
        assert_eq!(copied[0].source, "branch_merge");
        let pending = pending_merge_context(&db.conn, source_id).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].1.contains("Branch conclusion"));
        assert!(pending[0].1.contains("final.pdf"));
        assert!(pending[0].1.contains("notes.txt"));
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

        let merged_info = get_info(&db.conn, branch.id).await.unwrap().unwrap();
        assert_eq!(merged_info.lifecycle_state, "merged");
        assert_eq!(
            conversation_service::get_by_id(&db.conn, branch.id)
                .await
                .unwrap()
                .status,
            "completed"
        );
        let active = conversation_service::list_all(
            &db.conn,
            Some(vec![folder_id]),
            None,
            None,
            None,
            None,
            false,
        )
        .await
        .unwrap();
        assert!(active
            .iter()
            .any(|conversation| conversation.id == source_id));
        assert!(
            active
                .iter()
                .all(|conversation| conversation.id != branch.id),
            "merged branches stay auditable but disappear from active lists"
        );

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

    #[tokio::test]
    async fn branch_merge_history_projects_only_a_bounded_summary_preview() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-branch-preview-test").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let branch_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let summary = "回归正文".repeat(2_000);
        conversation_branch_merge::ActiveModel {
            id: Set("merge-large-summary".into()),
            branch_conversation_id: Set(branch_id),
            source_conversation_id: Set(source_id),
            target_conversation_id: Set(source_id),
            summary: Set(summary.clone()),
            deliverable_ids_json: Set("[]".into()),
            created_at: Set(Utc::now()),
            context_consumed_at: Set(None),
        }
        .insert(&db.conn)
        .await
        .unwrap();

        let page = merge_preview_page_for_target(&db.conn, source_id, 0, 20)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.items[0].summary_preview.chars().count() <= 512);
        assert_eq!(page.items[0].summary_bytes, summary.len() as i64);
        assert_ne!(page.items[0].summary_preview, summary);
        assert_eq!(
            merge_summary_for_target(&db.conn, source_id, "merge-large-summary")
                .await
                .unwrap()
                .as_deref(),
            Some(summary.as_str())
        );
    }

    #[tokio::test]
    async fn fourteen_branches_and_seven_returns_stay_summary_only() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-many-branches-test").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let source = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();
        let mut branch_ids = Vec::new();
        for index in 0..14 {
            let (branch, _) = create_branch_row(
                &db.conn,
                &source,
                Some(format!("branch-session-{index}")),
                None,
                "native",
                BranchInheritanceRecord {
                    source_session_id: source.external_id.clone(),
                    branch_session_id: Some(format!("branch-session-{index}")),
                    inheritance_mode: "native_fork".into(),
                    inherited_message_count: 20_000,
                    inherited_context_chars: 2_000_000_000,
                    inherited_estimated_tokens: 500_000_000,
                    inheritance_compressed: false,
                    inheritance_truncated: false,
                    inheritance_note: None,
                    forked_through_at: None,
                    source_rollout_offset: Some(2_000_000_000),
                    branch_rollout_offset: Some(2_000_000_000),
                    fork_boundary_kind: Some("exact_rollout_offset".into()),
                    snapshot_version: 2,
                    snapshot_context: None,
                    snapshot_images: Vec::new(),
                },
            )
            .await
            .unwrap();
            branch_ids.push(branch.id);
        }
        for (index, branch_id) in branch_ids.iter().take(7).enumerate() {
            conversation_branch_merge::ActiveModel {
                id: Set(format!("many-merge-{index}")),
                branch_conversation_id: Set(*branch_id),
                source_conversation_id: Set(source_id),
                target_conversation_id: Set(source_id),
                summary: Set(format!("summary-{index}")),
                deliverable_ids_json: Set("[]".into()),
                created_at: Set(Utc::now() + chrono::Duration::seconds(index as i64)),
                context_consumed_at: Set(Some(Utc::now())),
            }
            .insert(&db.conn)
            .await
            .unwrap();
        }

        let started = std::time::Instant::now();
        let branches = branch_preview_page_for_source(&db.conn, source_id, 0, 20)
            .await
            .unwrap();
        let merges = merge_preview_page_for_target(&db.conn, source_id, 0, 20)
            .await
            .unwrap();
        assert_eq!(branches.items.len(), 14);
        assert!(!branches.has_more);
        assert_eq!(merges.items.len(), 7);
        assert!(merges.items.iter().all(|item| item.summary_bytes < 64));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[tokio::test]
    async fn merge_reuses_existing_target_deliverable_instead_of_violating_unique_key() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "D:/codeg/merge-conflict").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        conversation_service::bind_external_id(&db.conn, source_id, "source-session-stable", &[])
            .await
            .unwrap();
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
                source_rollout_offset: None,
                branch_rollout_offset: None,
                fork_boundary_kind: None,
                snapshot_version: 2,
                snapshot_context: None,
                snapshot_images: Vec::new(),
            },
        )
        .await
        .unwrap();
        let now = Utc::now();
        for (id, conversation_id) in [("source-existing", source_id), ("branch-new", branch.id)] {
            conversation_deliverable::ActiveModel {
                id: Set(id.into()),
                conversation_id: Set(conversation_id),
                turn_run_id: Set(None),
                root_path: Set("D:/codeg/merge-conflict".into()),
                path: Set("STATUS.md".into()),
                kind: Set("file".into()),
                title: Set("STATUS.md".into()),
                description: Set(None),
                role: Set("supporting".into()),
                category: Set("document".into()),
                change_kind: Set("modified".into()),
                position: Set(0),
                source: Set("declared".into()),
                file_name: Set("STATUS.md".into()),
                extension: Set(Some("md".into())),
                size_bytes: Set(Some(42)),
                modified_at: Set(Some(now)),
                is_valid: Set(true),
                invalid_reason: Set(None),
                is_hidden: Set(false),
                verified_at: Set(now),
                last_checked_at: Set(Some(now)),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&db.conn)
            .await
            .unwrap();
        }

        let merged = merge_branch(
            &db.conn,
            branch.id,
            "merge-constraint".into(),
            "Branch conclusion".into(),
            Vec::new(),
        )
        .await
        .expect("the existing durable file identity should be reused");
        assert_eq!(merged.copied_deliverable_count, 1);
        let target_rows = conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(source_id))
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(target_rows.len(), 1);
        assert_eq!(target_rows[0].id, "source-existing");
        assert_eq!(target_rows[0].source, "branch_merge");
        let audit = conversation_branch_merge::Entity::find_by_id("merge-constraint")
            .one(&db.conn)
            .await
            .unwrap()
            .expect("merge audit");
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&audit.deliverable_ids_json).unwrap(),
            vec!["source-existing"]
        );
        let association = conversation_turn_deliverable::Entity::find()
            .filter(
                conversation_turn_deliverable::Column::TurnRunId
                    .eq("branch-merge-merge-constraint"),
            )
            .one(&db.conn)
            .await
            .unwrap()
            .expect("merge version association");
        assert_eq!(association.deliverable_id, "source-existing");
        assert_eq!(
            conversation_service::get_by_id(&db.conn, source_id)
                .await
                .unwrap()
                .external_id
                .as_deref(),
            Some("source-session-stable")
        );
    }

    #[tokio::test]
    async fn merge_reuses_windows_path_aliases_and_keeps_one_durable_identity() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "D:/codeg/merge-alias").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let branch = seed_native_test_branch(&db, source_id).await;
        seed_test_deliverable(
            &db.conn,
            TestDeliverable {
                id: "legacy-target",
                conversation_id: source_id,
                root_path: "D:\\codeg\\merge-alias",
                path: "Exports\\Final.CSV",
                source: "declared",
                valid: true,
                hidden: false,
                change_kind: "created",
                category: "standalone_output",
            },
        )
        .await;
        seed_test_deliverable(
            &db.conn,
            TestDeliverable {
                id: "branch-version",
                conversation_id: branch.id,
                root_path: "d:/codeg/./merge-alias/",
                path: "exports/draft/../final.csv",
                source: "declared",
                valid: true,
                hidden: false,
                change_kind: "modified",
                category: "standalone_output",
            },
        )
        .await;

        let result = merge_branch(
            &db.conn,
            branch.id,
            "merge-windows-alias".into(),
            "Updated report".into(),
            Vec::new(),
        )
        .await
        .unwrap();
        assert_eq!(result.copied_deliverable_count, 1);
        let target_rows = conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(source_id))
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(target_rows.len(), 1);
        assert_eq!(target_rows[0].id, "legacy-target");
        assert_eq!(target_rows[0].title, "title-branch-version");
        assert_eq!(target_rows[0].change_kind, "modified");
    }

    #[tokio::test]
    async fn merge_filters_invalid_hidden_deleted_and_transient_inferred_outputs() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "D:/codeg/merge-filter").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let branch = seed_native_test_branch(&db, source_id).await;
        for (id, path, source, valid, hidden, change_kind, category) in [
            (
                "declared-final",
                "deliverables/final.txt",
                "declared",
                true,
                false,
                "created",
                "standalone_output",
            ),
            (
                "invalid",
                "deliverables/missing.txt",
                "declared",
                false,
                false,
                "created",
                "standalone_output",
            ),
            (
                "hidden",
                "deliverables/hidden.txt",
                "declared",
                true,
                true,
                "created",
                "standalone_output",
            ),
            (
                "deleted",
                "deliverables/old.txt",
                "declared",
                true,
                false,
                "deleted",
                "standalone_output",
            ),
            (
                "qa-preview",
                "_qa/page-01.png",
                "inferred",
                true,
                false,
                "created",
                "standalone_output",
            ),
            (
                "inferred-duplicate",
                "deliverables\\final.txt",
                "inferred",
                true,
                false,
                "created",
                "standalone_output",
            ),
        ] {
            seed_test_deliverable(
                &db.conn,
                TestDeliverable {
                    id,
                    conversation_id: branch.id,
                    root_path: "D:/codeg/merge-filter",
                    path,
                    source,
                    valid,
                    hidden,
                    change_kind,
                    category,
                },
            )
            .await;
        }

        let result = merge_branch(
            &db.conn,
            branch.id,
            "merge-filtered".into(),
            "Only current user output".into(),
            Vec::new(),
        )
        .await
        .unwrap();
        assert_eq!(result.copied_deliverable_count, 1);
        let targets = conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(source_id))
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].path, "deliverables/final.txt");
        assert_eq!(targets[0].title, "title-declared-final");
    }

    #[tokio::test]
    async fn merge_database_failure_rolls_back_every_write_and_preserves_branch() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "D:/codeg/merge-rollback").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let source_before = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();
        let branch = seed_native_test_branch(&db, source_id).await;
        seed_test_deliverable(
            &db.conn,
            TestDeliverable {
                id: "branch-output",
                conversation_id: branch.id,
                root_path: "D:/codeg/merge-rollback",
                path: "result.txt",
                source: "declared",
                valid: true,
                hidden: false,
                change_kind: "created",
                category: "standalone_output",
            },
        )
        .await;
        db.conn
            .execute(Statement::from_string(
                DatabaseBackend::Sqlite,
                "CREATE TRIGGER inject_merge_failure BEFORE INSERT ON conversation_branch_merge BEGIN SELECT RAISE(ABORT, 'injected merge failure'); END".to_string(),
            ))
            .await
            .unwrap();

        let error = merge_branch(
            &db.conn,
            branch.id,
            "merge-rollback".into(),
            "Must roll back".into(),
            Vec::new(),
        )
        .await
        .expect_err("injected SQLite failure");
        assert!(error.to_string().contains("injected merge failure"));
        assert!(
            conversation_branch_merge::Entity::find_by_id("merge-rollback")
                .one(&db.conn)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            conversation_turn_run::Entity::find_by_id("branch-merge-merge-rollback")
                .one(&db.conn)
                .await
                .unwrap()
                .is_none()
        );
        assert!(conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(source_id))
            .all(&db.conn)
            .await
            .unwrap()
            .is_empty());
        let relation = get_info(&db.conn, branch.id).await.unwrap().unwrap();
        assert_ne!(relation.lifecycle_state, "merged");
        assert_eq!(
            conversation_service::get_by_id(&db.conn, source_id)
                .await
                .unwrap()
                .message_count,
            source_before.message_count
        );
    }

    #[tokio::test]
    async fn concurrent_merge_requests_commit_exactly_one_audit_event() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "D:/codeg/merge-concurrent").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let branch = seed_native_test_branch(&db, source_id).await;

        let (first, second) = tokio::join!(
            merge_branch(
                &db.conn,
                branch.id,
                "merge-concurrent-a".into(),
                "Concurrent result".into(),
                Vec::new(),
            ),
            merge_branch(
                &db.conn,
                branch.id,
                "merge-concurrent-b".into(),
                "Concurrent duplicate".into(),
                Vec::new(),
            )
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first.merge_id, second.merge_id);
        assert_ne!(first.deduplicated, second.deduplicated);
        assert_eq!(
            conversation_branch_merge::Entity::find()
                .filter(conversation_branch_merge::Column::BranchConversationId.eq(branch.id),)
                .count(&db.conn)
                .await
                .unwrap(),
            1
        );
    }
}
