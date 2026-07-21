use std::collections::HashMap;

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder, Set, TransactionTrait,
};

use crate::db::entities::conversation_turn_file_change::{
    self, ConversationTurnFileChangeKind,
};
use crate::db::entities::conversation_turn_run::{self, ConversationTurnRunStatus};
use crate::db::error::DbError;
use crate::models::{ConversationTurnArtifactRun, ConversationTurnFileChange};

#[derive(Debug, Clone)]
pub struct NewTurnRun {
    pub id: String,
    pub conversation_id: i32,
    pub connection_id: String,
    pub client_message_id: Option<String>,
    pub folder_id: Option<i32>,
    pub root_path: String,
    pub capture_incomplete: bool,
    pub input_paths_json: String,
}

#[derive(Debug, Clone)]
pub struct PendingFileChange {
    pub path: String,
    pub kind: ConversationTurnFileChangeKind,
    pub attribution: String,
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
        folder_id: Set(input.folder_id),
        root_path: Set(input.root_path),
        status: Set(ConversationTurnRunStatus::Running),
        capture_incomplete: Set(input.capture_incomplete),
        stop_reason: Set(None),
        started_at: Set(now),
        completed_at: Set(None),
        deliverables_declared_at: Set(None),
        input_paths_json: Set(input.input_paths_json),
    }
    .insert(conn)
    .await?;
    Ok(model)
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
            .filter(
                conversation_turn_file_change::Column::TurnRunId.eq(turn_run_id.to_string()),
            )
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

pub async fn mark_run_ambiguous(
    conn: &DatabaseConnection,
    run_id: &str,
) -> Result<(), DbError> {
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
    if model.status != ConversationTurnRunStatus::Running {
        return Ok(());
    }
    let mut active = model.into_active_model();
    active.status = Set(status);
    active.stop_reason = Set(stop_reason);
    active.completed_at = Set(Some(Utc::now()));
    active.update(conn).await?;
    Ok(())
}

/// Any `running` row predates this process: active captures live only in memory,
/// so after a restart no future event can complete them. Preserve their already
/// persisted paths but mark the capture explicitly incomplete/interrupted.
pub async fn recover_interrupted_runs(conn: &DatabaseConnection) -> Result<u64, DbError> {
    let rows = conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::Status.eq(ConversationTurnRunStatus::Running))
        .all(conn)
        .await?;
    let count = rows.len() as u64;
    for row in rows {
        let mut active = row.into_active_model();
        active.status = Set(ConversationTurnRunStatus::Interrupted);
        active.capture_incomplete = Set(true);
        active.stop_reason = Set(Some("app_restarted".to_string()));
        active.completed_at = Set(Some(Utc::now()));
        active.update(conn).await?;
    }
    Ok(count)
}

fn run_status_str(status: &ConversationTurnRunStatus) -> &'static str {
    match status {
        ConversationTurnRunStatus::Running => "running",
        ConversationTurnRunStatus::Completed => "completed",
        ConversationTurnRunStatus::Cancelled => "cancelled",
        ConversationTurnRunStatus::Interrupted => "interrupted",
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
            folder_id: run.folder_id,
            root_path: run.root_path,
            status: run_status_str(&run.status).to_string(),
            capture_incomplete: run.capture_incomplete,
            stop_reason: run.stop_reason,
            started_at: run.started_at,
            completed_at: run.completed_at,
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
                folder_id: Some(folder_id),
                root_path: "/tmp/artifacts".into(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
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
        assert_eq!(result[0].changes.len(), 1);
        assert_eq!(result[0].changes[0].kind, "created");
        assert_eq!(result[0].changes[0].event_count, 2);
        assert_eq!(result[0].changes[0].attribution, "ambiguous");
    }
}
