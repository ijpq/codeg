use std::sync::LazyLock;

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};

use crate::db::entities::conversation_branch_creation_task;
use crate::db::error::DbError;

static TASK_STATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static WORKER_INSTANCE_ID: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());

pub async fn create_or_get(
    conn: &DatabaseConnection,
    request_id: &str,
    source_conversation_id: i32,
    fork_message_id: Option<&str>,
) -> Result<conversation_branch_creation_task::Model, DbError> {
    let _guard = TASK_STATE_LOCK.lock().await;
    if let Some(existing) = conversation_branch_creation_task::Entity::find_by_id(request_id)
        .one(conn)
        .await?
    {
        if existing.source_conversation_id != source_conversation_id
            || existing.fork_message_id.as_deref() != fork_message_id
        {
            return Err(DbError::Validation(
                "branch request id already belongs to another operation".into(),
            ));
        }
        return Ok(existing);
    }
    let now = Utc::now();
    Ok(conversation_branch_creation_task::ActiveModel {
        request_id: Set(request_id.to_string()),
        source_conversation_id: Set(source_conversation_id),
        fork_message_id: Set(fork_message_id.map(str::to_string)),
        status: Set("queued".into()),
        stage: Set("queued".into()),
        result_json: Set(None),
        error: Set(None),
        cancel_requested: Set(false),
        worker_instance_id: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(conn)
    .await?)
}

pub async fn get(
    conn: &DatabaseConnection,
    request_id: &str,
) -> Result<Option<conversation_branch_creation_task::Model>, DbError> {
    Ok(
        conversation_branch_creation_task::Entity::find_by_id(request_id)
            .one(conn)
            .await?,
    )
}

/// Reconcile a row whose in-process worker disappeared during a server
/// restart. A durable task must never stay `running` forever or be presented
/// as live work after its owning process is gone.
pub async fn get_reconciled(
    conn: &DatabaseConnection,
    request_id: &str,
) -> Result<Option<conversation_branch_creation_task::Model>, DbError> {
    let _guard = TASK_STATE_LOCK.lock().await;
    let Some(row) = conversation_branch_creation_task::Entity::find_by_id(request_id)
        .one(conn)
        .await?
    else {
        return Ok(None);
    };
    if row.status != "running"
        || row.worker_instance_id.as_deref() == Some(WORKER_INSTANCE_ID.as_str())
    {
        return Ok(Some(row));
    }
    let mut active: conversation_branch_creation_task::ActiveModel = row.into();
    active.status = Set("failed".into());
    active.stage = Set("interrupted".into());
    active.error = Set(Some(
        "The server restarted while this branch was running. Check the conversation list for an already-created branch, then retry with a new request id if none exists."
            .into(),
    ));
    active.updated_at = Set(Utc::now());
    Ok(Some(active.update(conn).await?))
}

pub async fn mark_running(conn: &DatabaseConnection, request_id: &str) -> Result<bool, DbError> {
    let _guard = TASK_STATE_LOCK.lock().await;
    let Some(row) = get(conn, request_id).await? else {
        return Ok(false);
    };
    if row.status != "queued" || row.cancel_requested {
        return Ok(false);
    }
    let mut active: conversation_branch_creation_task::ActiveModel = row.into();
    active.status = Set("running".into());
    active.stage = Set("creating_branch".into());
    active.worker_instance_id = Set(Some(WORKER_INSTANCE_ID.clone()));
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(true)
}

pub async fn finish(
    conn: &DatabaseConnection,
    request_id: &str,
    result_json: Option<String>,
    error: Option<String>,
) -> Result<(), DbError> {
    let _guard = TASK_STATE_LOCK.lock().await;
    let Some(row) = get(conn, request_id).await? else {
        return Ok(());
    };
    let mut active: conversation_branch_creation_task::ActiveModel = row.into();
    let succeeded = error.is_none();
    active.status = Set(if succeeded { "succeeded" } else { "failed" }.into());
    active.stage = Set(if succeeded { "completed" } else { "failed" }.into());
    active.result_json = Set(result_json);
    active.error = Set(error);
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}

pub async fn cancel(
    conn: &DatabaseConnection,
    request_id: &str,
) -> Result<Option<conversation_branch_creation_task::Model>, DbError> {
    let _guard = TASK_STATE_LOCK.lock().await;
    let Some(row) = get(conn, request_id).await? else {
        return Ok(None);
    };
    if matches!(row.status.as_str(), "succeeded" | "failed" | "cancelled") {
        return Ok(Some(row));
    }
    let mut active: conversation_branch_creation_task::ActiveModel = row.into();
    active.cancel_requested = Set(true);
    // A queued task has not touched ACP state and can be cancelled exactly.
    // Running tasks retain their honest state until the worker reaches a safe
    // terminal boundary; the flag prevents a queued worker from starting.
    if active.status.as_ref() == "queued" {
        active.status = Set("cancelled".into());
        active.stage = Set("cancelled".into());
    }
    active.updated_at = Set(Utc::now());
    Ok(Some(active.update(conn).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AgentType;

    #[tokio::test]
    async fn task_state_survives_requery_and_duplicate_queueing() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let folder_id = crate::db::test_helpers::seed_folder(&db, "/tmp/branch-task").await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        let first = create_or_get(&db.conn, "stable-op", conversation_id, None)
            .await
            .unwrap();
        let duplicate = create_or_get(&db.conn, "stable-op", conversation_id, None)
            .await
            .unwrap();
        assert_eq!(first.request_id, duplicate.request_id);
        assert_eq!(duplicate.status, "queued");
        assert!(mark_running(&db.conn, "stable-op").await.unwrap());
        assert!(!mark_running(&db.conn, "stable-op").await.unwrap());
        finish(
            &db.conn,
            "stable-op",
            Some(r#"{"branchConversationId":9}"#.into()),
            None,
        )
        .await
        .unwrap();
        let restored = get(&db.conn, "stable-op").await.unwrap().unwrap();
        assert_eq!(restored.status, "succeeded");
        assert_eq!(restored.stage, "completed");
    }

    #[tokio::test]
    async fn queued_task_can_be_cancelled_without_starting() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let folder_id = crate::db::test_helpers::seed_folder(&db, "/tmp/branch-cancel").await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        create_or_get(&db.conn, "cancel-op", conversation_id, None)
            .await
            .unwrap();
        let cancelled = cancel(&db.conn, "cancel-op").await.unwrap().unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert!(cancelled.cancel_requested);
        assert!(!mark_running(&db.conn, "cancel-op").await.unwrap());
    }

    #[tokio::test]
    async fn running_task_from_an_old_server_instance_becomes_interrupted() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let folder_id = crate::db::test_helpers::seed_folder(&db, "/tmp/branch-restart").await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;
        create_or_get(&db.conn, "restart-op", conversation_id, None)
            .await
            .unwrap();
        assert!(mark_running(&db.conn, "restart-op").await.unwrap());

        let row = get(&db.conn, "restart-op").await.unwrap().unwrap();
        let mut active: conversation_branch_creation_task::ActiveModel = row.into();
        active.worker_instance_id = Set(Some("stopped-server".into()));
        active.update(&db.conn).await.unwrap();

        let restored = get_reconciled(&db.conn, "restart-op")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.status, "failed");
        assert_eq!(restored.stage, "interrupted");
        assert!(restored.error.unwrap().contains("server restarted"));
    }
}
