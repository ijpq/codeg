use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sea_orm::{
    sea_query::Expr, ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection,
    EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, Set, TransactionTrait,
};

use crate::db::entities::conversation::{self, ConversationStatus};
use crate::db::entities::conversation_turn_file_change::{self, ConversationTurnFileChangeKind};
use crate::db::entities::conversation_turn_run::{self, ConversationTurnRunStatus};
use crate::db::error::DbError;
use crate::models::{ConversationTurnArtifactRun, ConversationTurnFileChange};

#[derive(Debug, Clone)]
pub struct NewTurnRun {
    pub id: String,
    pub conversation_id: i32,
    pub connection_id: String,
    pub client_message_id: Option<String>,
    pub prompt_fingerprint: Option<String>,
    pub folder_id: Option<i32>,
    pub root_path: String,
    pub capture_incomplete: bool,
    pub input_paths_json: String,
    pub expectation_json: String,
}

#[derive(Debug, Clone)]
pub struct PendingFileChange {
    pub path: String,
    pub kind: ConversationTurnFileChangeKind,
    pub attribution: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelRequestDisposition {
    CancelRequested,
    AlreadyCancelling,
    AlreadyFinished,
    RunNotFound,
}

#[derive(Debug, Clone)]
pub struct CancelRequestTransition {
    pub disposition: CancelRequestDisposition,
    pub run: Option<conversation_turn_run::Model>,
}

/// Atomically close the user-visible turn lifecycle. File inspection and
/// deliverable inference deliberately happen after this transaction: they are
/// useful metadata, but a slow/locked artifact writer must never leave the
/// conversation looking active after the agent has already stopped.
pub async fn finalize_turn_state(
    conn: &DatabaseConnection,
    run_id: &str,
    run_status: ConversationTurnRunStatus,
    stop_reason: &str,
    conversation_status: ConversationStatus,
    capture_incomplete: bool,
    settle_incomplete: bool,
) -> Result<bool, DbError> {
    let txn = conn.begin().await?;
    let now = Utc::now();
    let Some(current) = conversation_turn_run::Entity::find_by_id(run_id.to_string())
        .one(&txn)
        .await?
    else {
        txn.rollback().await?;
        return Ok(false);
    };
    if !matches!(
        current.status,
        ConversationTurnRunStatus::Running | ConversationTurnRunStatus::Cancelling
    ) {
        txn.rollback().await?;
        return Ok(false);
    }
    // Cancellation owns the terminal transition. An agent may race a late
    // end_turn against the user's stop request; once the durable row entered
    // `cancelling`, that late event must not resurrect a successful turn.
    let cancellation_won = current.status == ConversationTurnRunStatus::Cancelling;
    let explicit_cancellation_finalizer = run_status == ConversationTurnRunStatus::Cancelled;
    let final_run_status = if cancellation_won {
        ConversationTurnRunStatus::Cancelled
    } else {
        run_status
    };
    let final_stop_reason = if cancellation_won && !explicit_cancellation_finalizer {
        // A late successful/failed agent event must not overwrite a durable
        // cancellation request.  Explicit cancellation finalizers, however,
        // carry useful auditable reasons such as `cancel_timeout` or
        // `cancelled_during_restart`; retain those.
        "cancelled"
    } else {
        stop_reason
    };
    let final_conversation_status = if cancellation_won {
        ConversationStatus::Cancelled
    } else {
        conversation_status
    };
    let final_capture_incomplete = capture_incomplete || cancellation_won;
    let final_settle_incomplete = settle_incomplete || cancellation_won;
    let mut update = conversation_turn_run::Entity::update_many()
        .col_expr(
            conversation_turn_run::Column::Status,
            Expr::value(final_run_status),
        )
        .col_expr(
            conversation_turn_run::Column::StopReason,
            Expr::value(Some(final_stop_reason.to_string())),
        )
        .col_expr(
            conversation_turn_run::Column::CompletedAt,
            Expr::value(Some(now)),
        )
        .filter(conversation_turn_run::Column::Id.eq(run_id.to_string()))
        .filter(conversation_turn_run::Column::Status.is_in([
            ConversationTurnRunStatus::Running,
            ConversationTurnRunStatus::Cancelling,
        ]));
    if final_capture_incomplete {
        update = update.col_expr(
            conversation_turn_run::Column::CaptureIncomplete,
            Expr::value(true),
        );
    }
    if final_settle_incomplete {
        update = update
            .col_expr(
                conversation_turn_run::Column::SettlementStatus,
                Expr::value("settled_incomplete"),
            )
            .col_expr(
                conversation_turn_run::Column::SettledAt,
                Expr::value(Some(now)),
            );
    }
    let changed = update.exec(&txn).await?.rows_affected == 1;
    if changed {
        conversation::Entity::update_many()
            .col_expr(
                conversation::Column::Status,
                Expr::value(final_conversation_status),
            )
            .col_expr(conversation::Column::UpdatedAt, Expr::value(now))
            .filter(conversation::Column::Id.eq(current.conversation_id))
            .exec(&txn)
            .await?;
    }
    txn.commit().await?;
    Ok(changed)
}

/// A terminal run may still be settling its optional filesystem metadata.
/// Admission uses this to queue the next prompt instead of superseding the old
/// capture and producing a second, contradictory run.
pub async fn has_unsettled_run(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<bool, DbError> {
    Ok(conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .filter(conversation_turn_run::Column::SettlementStatus.eq("pending"))
        .one(conn)
        .await?
        .is_some())
}

pub async fn create_run(
    conn: &DatabaseConnection,
    input: NewTurnRun,
) -> Result<conversation_turn_run::Model, DbError> {
    let now = Utc::now();
    let model = conversation_turn_run::ActiveModel {
        id: Set(input.id),
        conversation_id: Set(input.conversation_id),
        connection_id: Set(input.connection_id),
        client_message_id: Set(input.client_message_id),
        prompt_accepted_at: Set(None),
        prompt_fingerprint: Set(input.prompt_fingerprint),
        cancel_request_id: Set(None),
        cancel_requested_at: Set(None),
        cancel_deadline_at: Set(None),
        folder_id: Set(input.folder_id),
        root_path: Set(input.root_path),
        status: Set(ConversationTurnRunStatus::Running),
        capture_incomplete: Set(input.capture_incomplete),
        stop_reason: Set(None),
        started_at: Set(now),
        completed_at: Set(None),
        deliverables_declared_at: Set(None),
        input_paths_json: Set(input.input_paths_json),
        declaration_status: Set("not_called".to_string()),
        declaration_attempted_at: Set(None),
        declaration_error: Set(None),
        expectation_json: Set(input.expectation_json),
        settlement_status: Set("pending".to_string()),
        settled_at: Set(None),
        missing_expected_paths_json: Set("[]".to_string()),
    }
    .insert(conn)
    .await?;
    Ok(model)
}

pub async fn mark_prompt_accepted(conn: &DatabaseConnection, run_id: &str) -> Result<(), DbError> {
    let Some(model) = conversation_turn_run::Entity::find_by_id(run_id.to_string())
        .one(conn)
        .await?
    else {
        return Ok(());
    };
    if model.prompt_accepted_at.is_some() {
        return Ok(());
    }
    let mut active = model.into_active_model();
    active.prompt_accepted_at = Set(Some(Utc::now()));
    active.update(conn).await?;
    Ok(())
}

pub async fn was_prompt_accepted(
    conn: &DatabaseConnection,
    conversation_id: i32,
    client_message_id: &str,
) -> Result<bool, DbError> {
    Ok(conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .filter(conversation_turn_run::Column::ClientMessageId.eq(client_message_id.to_string()))
        .filter(conversation_turn_run::Column::PromptAcceptedAt.is_not_null())
        .one(conn)
        .await?
        .is_some())
}

fn merge_kind(
    current: ConversationTurnFileChangeKind,
    incoming: ConversationTurnFileChangeKind,
) -> ConversationTurnFileChangeKind {
    use ConversationTurnFileChangeKind::{Created, Deleted, Modified, Renamed};
    match (current, incoming) {
        // A path born during the turn remains a creation even when an editor
        // rewrites or briefly removes it. `final_exists=false` distinguishes a
        // transient create+delete at finalization time.
        (Created, _) => Created,
        // Atomic-save pattern: remove the old path, then create the replacement.
        (Deleted, Created | Modified | Renamed) => Modified,
        (_, Deleted) => Deleted,
        (_, Renamed) => Renamed,
        (_, Created) => Modified,
        (kind, Modified) => kind,
    }
}

pub async fn upsert_changes(
    conn: &DatabaseConnection,
    turn_run_id: &str,
    changes: Vec<PendingFileChange>,
) -> Result<(), DbError> {
    if changes.is_empty() {
        return Ok(());
    }

    let txn = conn.begin().await?;
    let now = Utc::now();
    for change in changes {
        let existing = conversation_turn_file_change::Entity::find()
            .filter(conversation_turn_file_change::Column::TurnRunId.eq(turn_run_id.to_string()))
            .filter(conversation_turn_file_change::Column::Path.eq(change.path.clone()))
            .one(&txn)
            .await?;

        if let Some(existing) = existing {
            let merged_kind = merge_kind(existing.kind, change.kind);
            let next_event_count = existing.event_count.saturating_add(1);
            let mut active = existing.into_active_model();
            active.kind = Set(merged_kind);
            active.last_seen_at = Set(now);
            active.event_count = Set(next_event_count);
            if change.attribution == "ambiguous" {
                active.attribution = Set(change.attribution);
            }
            active.update(&txn).await?;
        } else {
            conversation_turn_file_change::ActiveModel {
                id: NotSet,
                turn_run_id: Set(turn_run_id.to_string()),
                path: Set(change.path),
                old_path: Set(None),
                kind: Set(change.kind),
                source: Set("watcher".to_string()),
                attribution: Set(change.attribution),
                first_seen_at: Set(now),
                last_seen_at: Set(now),
                event_count: Set(1),
                final_exists: Set(None),
                size_bytes: Set(None),
                modified_at: Set(None),
            }
            .insert(&txn)
            .await?;
        }
    }
    txn.commit().await?;
    Ok(())
}

pub async fn mark_capture_incomplete(
    conn: &DatabaseConnection,
    run_id: &str,
) -> Result<(), DbError> {
    let Some(model) = conversation_turn_run::Entity::find_by_id(run_id.to_string())
        .one(conn)
        .await?
    else {
        return Ok(());
    };
    if model.capture_incomplete {
        return Ok(());
    }
    let mut active = model.into_active_model();
    active.capture_incomplete = Set(true);
    active.update(conn).await?;
    Ok(())
}

pub async fn mark_run_ambiguous(conn: &DatabaseConnection, run_id: &str) -> Result<(), DbError> {
    let rows = conversation_turn_file_change::Entity::find()
        .filter(conversation_turn_file_change::Column::TurnRunId.eq(run_id.to_string()))
        .all(conn)
        .await?;
    for row in rows {
        if row.attribution == "ambiguous" {
            continue;
        }
        let mut active = row.into_active_model();
        active.attribution = Set("ambiguous".to_string());
        active.update(conn).await?;
    }
    Ok(())
}

pub async fn list_changes_for_run(
    conn: &DatabaseConnection,
    run_id: &str,
) -> Result<Vec<conversation_turn_file_change::Model>, DbError> {
    Ok(conversation_turn_file_change::Entity::find()
        .filter(conversation_turn_file_change::Column::TurnRunId.eq(run_id.to_string()))
        .order_by_asc(conversation_turn_file_change::Column::FirstSeenAt)
        .all(conn)
        .await?)
}

pub async fn update_final_state(
    conn: &DatabaseConnection,
    change: conversation_turn_file_change::Model,
    final_exists: bool,
    size_bytes: Option<i64>,
    modified_at: Option<chrono::DateTime<Utc>>,
) -> Result<(), DbError> {
    let mut active = change.into_active_model();
    active.final_exists = Set(Some(final_exists));
    active.size_bytes = Set(size_bytes);
    active.modified_at = Set(modified_at);
    active.update(conn).await?;
    Ok(())
}

pub async fn delete_change(
    conn: &DatabaseConnection,
    change: conversation_turn_file_change::Model,
) -> Result<(), DbError> {
    conversation_turn_file_change::Entity::delete_by_id(change.id)
        .exec(conn)
        .await?;
    Ok(())
}

pub async fn finish_run(
    conn: &DatabaseConnection,
    run_id: &str,
    status: ConversationTurnRunStatus,
    stop_reason: Option<String>,
) -> Result<(), DbError> {
    let Some(model) = conversation_turn_run::Entity::find_by_id(run_id.to_string())
        .one(conn)
        .await?
    else {
        return Ok(());
    };
    if !matches!(
        model.status,
        ConversationTurnRunStatus::Running | ConversationTurnRunStatus::Cancelling
    ) {
        return Ok(());
    }
    // Cancellation gets first refusal. This must be two conditional writes,
    // not one update based on the row read above: cancel can win its
    // running->cancelling CAS between this SELECT and our terminal UPDATE.
    // In that race the running-only completion below affects zero rows, so a
    // late end_turn can never turn a cancellation back into success.
    let cancelled = conversation_turn_run::Entity::update_many()
        .col_expr(
            conversation_turn_run::Column::Status,
            Expr::value(ConversationTurnRunStatus::Cancelled),
        )
        .col_expr(
            conversation_turn_run::Column::StopReason,
            Expr::value(Some("cancelled".to_string())),
        )
        .col_expr(
            conversation_turn_run::Column::CompletedAt,
            Expr::value(Some(Utc::now())),
        )
        .filter(conversation_turn_run::Column::Id.eq(run_id.to_string()))
        .filter(conversation_turn_run::Column::Status.eq(ConversationTurnRunStatus::Cancelling))
        .exec(conn)
        .await?;
    if cancelled.rows_affected == 0 {
        conversation_turn_run::Entity::update_many()
            .col_expr(conversation_turn_run::Column::Status, Expr::value(status))
            .col_expr(
                conversation_turn_run::Column::StopReason,
                Expr::value(stop_reason),
            )
            .col_expr(
                conversation_turn_run::Column::CompletedAt,
                Expr::value(Some(Utc::now())),
            )
            .filter(conversation_turn_run::Column::Id.eq(run_id.to_string()))
            .filter(conversation_turn_run::Column::Status.eq(ConversationTurnRunStatus::Running))
            .exec(conn)
            .await?;
    }
    Ok(())
}

/// Atomically claim cancellation for the active run on `connection_id`.
/// The conditional UPDATE is the cross-tab/process-local idempotency boundary:
/// only the caller that changes running -> cancelling may notify the agent or
/// arm a timeout. Every duplicate receives the stored first request receipt.
pub async fn request_cancel(
    conn: &DatabaseConnection,
    connection_id: &str,
    cancel_request_id: &str,
    requested_at: DateTime<Utc>,
    deadline_at: DateTime<Utc>,
) -> Result<CancelRequestTransition, DbError> {
    let active = conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConnectionId.eq(connection_id.to_string()))
        .filter(conversation_turn_run::Column::Status.is_in([
            ConversationTurnRunStatus::Running,
            ConversationTurnRunStatus::Cancelling,
        ]))
        .order_by_desc(conversation_turn_run::Column::StartedAt)
        .one(conn)
        .await?;

    let Some(active) = active else {
        let last = conversation_turn_run::Entity::find()
            .filter(conversation_turn_run::Column::ConnectionId.eq(connection_id.to_string()))
            .order_by_desc(conversation_turn_run::Column::StartedAt)
            .one(conn)
            .await?;
        return Ok(CancelRequestTransition {
            disposition: if last.is_some() {
                CancelRequestDisposition::AlreadyFinished
            } else {
                CancelRequestDisposition::RunNotFound
            },
            run: last,
        });
    };

    if active.status == ConversationTurnRunStatus::Cancelling {
        return Ok(CancelRequestTransition {
            disposition: CancelRequestDisposition::AlreadyCancelling,
            run: Some(active),
        });
    }

    let result = conversation_turn_run::Entity::update_many()
        .col_expr(
            conversation_turn_run::Column::Status,
            Expr::value(ConversationTurnRunStatus::Cancelling),
        )
        .col_expr(
            conversation_turn_run::Column::CaptureIncomplete,
            Expr::value(true),
        )
        .col_expr(
            conversation_turn_run::Column::CancelRequestId,
            Expr::value(Some(cancel_request_id.to_string())),
        )
        .col_expr(
            conversation_turn_run::Column::CancelRequestedAt,
            Expr::value(Some(requested_at)),
        )
        .col_expr(
            conversation_turn_run::Column::CancelDeadlineAt,
            Expr::value(Some(deadline_at)),
        )
        .filter(conversation_turn_run::Column::Id.eq(active.id.clone()))
        .filter(conversation_turn_run::Column::Status.eq(ConversationTurnRunStatus::Running))
        .exec(conn)
        .await?;

    let updated = conversation_turn_run::Entity::find_by_id(active.id)
        .one(conn)
        .await?;
    Ok(CancelRequestTransition {
        disposition: if result.rows_affected == 1 {
            CancelRequestDisposition::CancelRequested
        } else if updated
            .as_ref()
            .is_some_and(|run| run.status == ConversationTurnRunStatus::Cancelling)
        {
            CancelRequestDisposition::AlreadyCancelling
        } else {
            CancelRequestDisposition::AlreadyFinished
        },
        run: updated,
    })
}

pub async fn latest_run_for_connection(
    conn: &DatabaseConnection,
    connection_id: &str,
) -> Result<Option<conversation_turn_run::Model>, DbError> {
    Ok(conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConnectionId.eq(connection_id.to_string()))
        .order_by_desc(conversation_turn_run::Column::StartedAt)
        .one(conn)
        .await?)
}

pub async fn active_runs_for_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<conversation_turn_run::Model>, DbError> {
    Ok(conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .filter(conversation_turn_run::Column::Status.is_in([
            ConversationTurnRunStatus::Running,
            ConversationTurnRunStatus::Cancelling,
        ]))
        .order_by_desc(conversation_turn_run::Column::StartedAt)
        .all(conn)
        .await?)
}

pub async fn list_active_runs(
    conn: &DatabaseConnection,
) -> Result<Vec<conversation_turn_run::Model>, DbError> {
    Ok(conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::Status.is_in([
            ConversationTurnRunStatus::Running,
            ConversationTurnRunStatus::Cancelling,
        ]))
        .order_by_asc(conversation_turn_run::Column::StartedAt)
        .all(conn)
        .await?)
}

/// Terminal fallback for a run whose in-memory capture owner is gone. This is
/// deliberately conservative: no automatic deliverable inference runs for an
/// interrupted/cancelled capture, so half-written files cannot become apparent
/// successful output. Explicit declarations already persisted remain intact.
pub async fn force_finish_incomplete(
    conn: &DatabaseConnection,
    run_id: &str,
    status: ConversationTurnRunStatus,
    stop_reason: &str,
) -> Result<bool, DbError> {
    let now = Utc::now();
    let result = conversation_turn_run::Entity::update_many()
        .col_expr(conversation_turn_run::Column::Status, Expr::value(status))
        .col_expr(
            conversation_turn_run::Column::CaptureIncomplete,
            Expr::value(true),
        )
        .col_expr(
            conversation_turn_run::Column::StopReason,
            Expr::value(Some(stop_reason.to_string())),
        )
        .col_expr(
            conversation_turn_run::Column::CompletedAt,
            Expr::value(Some(now)),
        )
        .col_expr(
            conversation_turn_run::Column::SettlementStatus,
            Expr::value("settled_incomplete"),
        )
        .col_expr(
            conversation_turn_run::Column::SettledAt,
            Expr::value(Some(now)),
        )
        .filter(conversation_turn_run::Column::Id.eq(run_id.to_string()))
        .filter(conversation_turn_run::Column::Status.is_in([
            ConversationTurnRunStatus::Running,
            ConversationTurnRunStatus::Cancelling,
        ]))
        .exec(conn)
        .await?;
    Ok(result.rows_affected == 1)
}

pub async fn mark_settled(
    conn: &DatabaseConnection,
    run_id: &str,
    status: &str,
    missing_expected_paths: &[String],
) -> Result<(), DbError> {
    conversation_turn_run::Entity::update_many()
        .col_expr(
            conversation_turn_run::Column::SettlementStatus,
            Expr::value(status.to_string()),
        )
        .col_expr(
            conversation_turn_run::Column::SettledAt,
            Expr::value(Some(Utc::now())),
        )
        .col_expr(
            conversation_turn_run::Column::MissingExpectedPathsJson,
            Expr::value(
                serde_json::to_string(missing_expected_paths).unwrap_or_else(|_| "[]".to_string()),
            ),
        )
        .filter(conversation_turn_run::Column::Id.eq(run_id.to_string()))
        .filter(conversation_turn_run::Column::SettlementStatus.eq("pending"))
        .exec(conn)
        .await?;
    Ok(())
}

/// Any `running` row predates this process: active captures live only in memory,
/// so after a restart no future event can complete them. Preserve their already
/// persisted paths but mark the capture explicitly incomplete/interrupted.
pub async fn recover_interrupted_runs(conn: &DatabaseConnection) -> Result<u64, DbError> {
    let rows = conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::Status.is_in([
            ConversationTurnRunStatus::Running,
            ConversationTurnRunStatus::Cancelling,
        ]))
        .all(conn)
        .await?;
    let mut count = 0;
    for row in rows {
        let was_cancelling = row.status == ConversationTurnRunStatus::Cancelling;
        if finalize_turn_state(
            conn,
            &row.id,
            if was_cancelling {
                ConversationTurnRunStatus::Cancelled
            } else {
                ConversationTurnRunStatus::Interrupted
            },
            if was_cancelling {
                "cancelled_during_restart"
            } else {
                "app_restarted"
            },
            if was_cancelling {
                ConversationStatus::Cancelled
            } else {
                ConversationStatus::PendingReview
            },
            true,
            true,
        )
        .await?
        {
            count += 1;
        }
    }
    Ok(count)
}

fn run_status_str(status: &ConversationTurnRunStatus) -> &'static str {
    match status {
        ConversationTurnRunStatus::Running => "running",
        ConversationTurnRunStatus::Cancelling => "cancelling",
        ConversationTurnRunStatus::Completed => "completed",
        ConversationTurnRunStatus::Cancelled => "cancelled",
        ConversationTurnRunStatus::Interrupted => "interrupted",
        ConversationTurnRunStatus::Failed => "failed",
    }
}

fn change_kind_str(kind: &ConversationTurnFileChangeKind) -> &'static str {
    match kind {
        ConversationTurnFileChangeKind::Created => "created",
        ConversationTurnFileChangeKind::Modified => "modified",
        ConversationTurnFileChangeKind::Deleted => "deleted",
        ConversationTurnFileChangeKind::Renamed => "renamed",
    }
}

fn change_to_info(model: conversation_turn_file_change::Model) -> ConversationTurnFileChange {
    ConversationTurnFileChange {
        id: model.id,
        path: model.path,
        old_path: model.old_path,
        kind: change_kind_str(&model.kind).to_string(),
        source: model.source,
        attribution: model.attribution,
        first_seen_at: model.first_seen_at,
        last_seen_at: model.last_seen_at,
        event_count: model.event_count,
        final_exists: model.final_exists,
        size_bytes: model.size_bytes,
        modified_at: model.modified_at,
    }
}

pub async fn list_for_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<ConversationTurnArtifactRun>, DbError> {
    let runs = conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .order_by_asc(conversation_turn_run::Column::StartedAt)
        .all(conn)
        .await?;
    if runs.is_empty() {
        return Ok(Vec::new());
    }

    let run_ids = runs.iter().map(|run| run.id.clone()).collect::<Vec<_>>();
    let changes = conversation_turn_file_change::Entity::find()
        .filter(conversation_turn_file_change::Column::TurnRunId.is_in(run_ids))
        .order_by_asc(conversation_turn_file_change::Column::FirstSeenAt)
        .all(conn)
        .await?;
    let mut by_run: HashMap<String, Vec<ConversationTurnFileChange>> = HashMap::new();
    for change in changes {
        by_run
            .entry(change.turn_run_id.clone())
            .or_default()
            .push(change_to_info(change));
    }

    Ok(runs
        .into_iter()
        .map(|run| ConversationTurnArtifactRun {
            changes: by_run.remove(&run.id).unwrap_or_default(),
            id: run.id,
            conversation_id: run.conversation_id,
            connection_id: run.connection_id,
            client_message_id: run.client_message_id,
            prompt_accepted_at: run.prompt_accepted_at,
            folder_id: run.folder_id,
            root_path: run.root_path,
            status: run_status_str(&run.status).to_string(),
            capture_incomplete: run.capture_incomplete,
            stop_reason: run.stop_reason,
            started_at: run.started_at,
            completed_at: run.completed_at,
            cancel_request_id: run.cancel_request_id,
            cancel_requested_at: run.cancel_requested_at,
            cancel_deadline_at: run.cancel_deadline_at,
            declaration_status: run.declaration_status,
            declaration_attempted_at: run.declaration_attempted_at,
            deliverables_declared_at: run.deliverables_declared_at,
            declaration_error: run.declaration_error,
            expectation_json: run.expectation_json,
            settlement_status: run.settlement_status,
            settled_at: run.settled_at,
            missing_expected_paths: serde_json::from_str(&run.missing_expected_paths_json)
                .unwrap_or_default(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AgentType;

    #[tokio::test]
    async fn aggregates_repeated_path_events_and_recovers_running_rows() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let folder_id = crate::db::test_helpers::seed_folder(&db, "/tmp/artifacts").await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;

        create_run(
            &db.conn,
            NewTurnRun {
                id: "run-1".into(),
                conversation_id,
                connection_id: "conn-1".into(),
                client_message_id: Some("optimistic-1".into()),
                prompt_fingerprint: None,
                folder_id: Some(folder_id),
                root_path: "/tmp/artifacts".into(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
                expectation_json:
                    r#"{"publish_required":true,"expects_code_changes":true,"requested_paths":[]}"#
                        .into(),
            },
        )
        .await
        .expect("run");
        upsert_changes(
            &db.conn,
            "run-1",
            vec![PendingFileChange {
                path: "report.docx".into(),
                kind: ConversationTurnFileChangeKind::Created,
                attribution: "exclusive".into(),
            }],
        )
        .await
        .expect("first change");
        upsert_changes(
            &db.conn,
            "run-1",
            vec![PendingFileChange {
                path: "report.docx".into(),
                kind: ConversationTurnFileChangeKind::Modified,
                attribution: "ambiguous".into(),
            }],
        )
        .await
        .expect("second change");

        let recovered = recover_interrupted_runs(&db.conn).await.expect("recover");
        assert_eq!(recovered, 1);
        let result = list_for_conversation(&db.conn, conversation_id)
            .await
            .expect("list");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status, "interrupted");
        assert!(result[0].capture_incomplete);
        assert_eq!(result[0].settlement_status, "settled_incomplete");
        assert_eq!(result[0].changes.len(), 1);
        assert_eq!(result[0].changes[0].kind, "created");
        assert_eq!(result[0].changes[0].event_count, 2);
        assert_eq!(result[0].changes[0].attribution, "ambiguous");
    }

    async fn seed_cancel_run(db: &crate::db::AppDatabase, connection_id: &str) -> String {
        let folder_id = crate::db::test_helpers::seed_folder(db, "/tmp/cancel-state").await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(db, folder_id, AgentType::Codex).await;
        let run_id = format!("run-{connection_id}");
        create_run(
            &db.conn,
            NewTurnRun {
                id: run_id.clone(),
                conversation_id,
                connection_id: connection_id.into(),
                client_message_id: Some(format!("message-{connection_id}")),
                prompt_fingerprint: None,
                folder_id: Some(folder_id),
                root_path: "/tmp/cancel-state".into(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
                expectation_json: "{}".into(),
            },
        )
        .await
        .expect("run");
        run_id
    }

    #[tokio::test]
    async fn concurrent_cancel_claim_is_idempotent_and_preserves_first_receipt() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let run_id = seed_cancel_run(&db, "conn-cancel").await;
        let now = Utc::now();
        let deadline = now + chrono::Duration::seconds(25);

        let (first, second) = tokio::join!(
            request_cancel(&db.conn, "conn-cancel", "request-one", now, deadline),
            request_cancel(&db.conn, "conn-cancel", "request-two", now, deadline)
        );
        let dispositions = [
            first.as_ref().unwrap().disposition.clone(),
            second.as_ref().unwrap().disposition.clone(),
        ];
        assert_eq!(
            dispositions
                .iter()
                .filter(|item| **item == CancelRequestDisposition::CancelRequested)
                .count(),
            1
        );
        assert_eq!(
            dispositions
                .iter()
                .filter(|item| **item == CancelRequestDisposition::AlreadyCancelling)
                .count(),
            1
        );
        let run = conversation_turn_run::Entity::find_by_id(run_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.status, ConversationTurnRunStatus::Cancelling);
        assert!(matches!(
            run.cancel_request_id.as_deref(),
            Some("request-one") | Some("request-two")
        ));
        assert!(run.capture_incomplete);
    }

    #[tokio::test]
    async fn late_success_cannot_override_a_claimed_cancellation() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let run_id = seed_cancel_run(&db, "conn-race").await;
        let now = Utc::now();
        request_cancel(
            &db.conn,
            "conn-race",
            "request-race",
            now,
            now + chrono::Duration::seconds(25),
        )
        .await
        .unwrap();

        finish_run(
            &db.conn,
            &run_id,
            ConversationTurnRunStatus::Completed,
            Some("end_turn".into()),
        )
        .await
        .unwrap();
        let run = conversation_turn_run::Entity::find_by_id(run_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.status, ConversationTurnRunStatus::Cancelled);
        assert!(run.completed_at.is_some());
    }

    #[tokio::test]
    async fn terminal_transaction_closes_run_settlement_and_conversation_together() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let run_id = seed_cancel_run(&db, "conn-atomic").await;
        let run = conversation_turn_run::Entity::find_by_id(run_id.clone())
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();

        assert!(finalize_turn_state(
            &db.conn,
            &run_id,
            ConversationTurnRunStatus::Interrupted,
            "connection_lost",
            ConversationStatus::PendingReview,
            true,
            true,
        )
        .await
        .unwrap());

        let closed = conversation_turn_run::Entity::find_by_id(run_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        let conversation = conversation::Entity::find_by_id(run.conversation_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(closed.status, ConversationTurnRunStatus::Interrupted);
        assert_eq!(closed.settlement_status, "settled_incomplete");
        assert!(closed.completed_at.is_some());
        assert!(closed.settled_at.is_some());
        assert_eq!(conversation.status, ConversationStatus::PendingReview);
    }

    #[tokio::test]
    async fn failed_turn_is_distinct_from_cancelled_and_interrupted() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let run_id = seed_cancel_run(&db, "conn-failed").await;

        finish_run(
            &db.conn,
            &run_id,
            ConversationTurnRunStatus::Failed,
            Some("max_tokens".into()),
        )
        .await
        .unwrap();

        let run = conversation_turn_run::Entity::find_by_id(run_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.status, ConversationTurnRunStatus::Failed);
        assert_eq!(run.stop_reason.as_deref(), Some("max_tokens"));
        assert!(run.completed_at.is_some());
    }

    #[tokio::test]
    async fn restart_closes_cancelling_run_without_leaving_pending_settlement() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let run_id = seed_cancel_run(&db, "conn-restart").await;
        let now = Utc::now();
        request_cancel(
            &db.conn,
            "conn-restart",
            "request-restart",
            now,
            now + chrono::Duration::seconds(25),
        )
        .await
        .unwrap();

        assert_eq!(recover_interrupted_runs(&db.conn).await.unwrap(), 1);
        let run = conversation_turn_run::Entity::find_by_id(run_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.status, ConversationTurnRunStatus::Cancelled);
        assert_eq!(run.stop_reason.as_deref(), Some("cancelled_during_restart"));
        assert_eq!(run.settlement_status, "settled_incomplete");
        assert!(run.completed_at.is_some());
        assert!(run.settled_at.is_some());
        let conversation = conversation::Entity::find_by_id(run.conversation_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(conversation.status, ConversationStatus::Cancelled);
    }
}
