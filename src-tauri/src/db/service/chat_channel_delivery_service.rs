use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};

use crate::db::entities::{chat_channel_outbox, chat_channel_turn_origin, conversation_turn_run};
use crate::db::error::DbError;

#[derive(Debug, Clone)]
pub struct NewTurnOrigin {
    pub channel_id: i32,
    pub sender_id: String,
    pub conversation_id: i32,
    pub connection_id: Option<String>,
    pub origin_message_id: String,
    pub client_message_id: String,
    pub target_json: String,
    pub prompt_json: Option<String>,
}

pub async fn register_origin(
    conn: &DatabaseConnection,
    input: NewTurnOrigin,
) -> Result<chat_channel_turn_origin::Model, DbError> {
    if let Some(existing) = chat_channel_turn_origin::Entity::find()
        .filter(chat_channel_turn_origin::Column::ChannelId.eq(input.channel_id))
        .filter(chat_channel_turn_origin::Column::SenderId.eq(input.sender_id.clone()))
        .filter(
            chat_channel_turn_origin::Column::OriginMessageId.eq(input.origin_message_id.clone()),
        )
        .one(conn)
        .await?
    {
        if existing.prompt_json.is_none()
            && input.prompt_json.is_some()
            && matches!(existing.status.as_str(), "queued" | "dispatch_failed")
        {
            let mut active = existing.into_active_model();
            active.prompt_json = Set(input.prompt_json);
            active.updated_at = Set(Utc::now());
            return Ok(active.update(conn).await?);
        }
        return Ok(existing);
    }
    let now = Utc::now();
    Ok(chat_channel_turn_origin::ActiveModel {
        id: Set(uuid::Uuid::new_v4().to_string()),
        channel_id: Set(input.channel_id),
        sender_id: Set(input.sender_id),
        conversation_id: Set(input.conversation_id),
        connection_id: Set(input.connection_id),
        origin_message_id: Set(input.origin_message_id),
        client_message_id: Set(input.client_message_id),
        turn_run_id: Set(None),
        target_json: Set(input.target_json),
        prompt_json: Set(input.prompt_json),
        status: Set("queued".to_string()),
        attempt_count: Set(0),
        last_error: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        final_captured_at: Set(None),
    }
    .insert(conn)
    .await?)
}

pub async fn mark_dispatched(
    conn: &DatabaseConnection,
    origin_id: &str,
    connection_id: &str,
) -> Result<chat_channel_turn_origin::Model, DbError> {
    let Some(model) = chat_channel_turn_origin::Entity::find_by_id(origin_id.to_string())
        .one(conn)
        .await?
    else {
        return Err(DbError::NotFound("chat channel turn origin".to_string()));
    };
    let turn_run_id = conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConversationId.eq(model.conversation_id))
        .filter(conversation_turn_run::Column::ClientMessageId.eq(model.client_message_id.clone()))
        .order_by_desc(conversation_turn_run::Column::StartedAt)
        .one(conn)
        .await?
        .map(|run| run.id);
    let mut active = model.into_active_model();
    active.connection_id = Set(Some(connection_id.to_string()));
    active.turn_run_id = Set(turn_run_id);
    active.status = Set("dispatched".to_string());
    active.last_error = Set(None);
    active.updated_at = Set(Utc::now());
    let updated = active.update(conn).await?;
    for message_type in ["task_dispatched", "turn_started"] {
        let _ = super::chat_channel_message_log_service::create_correlated_log(
            conn,
            updated.channel_id,
            "internal",
            message_type,
            "",
            "accepted",
            None,
            Some(&updated.origin_message_id),
            updated.turn_run_id.as_deref(),
            None,
        )
        .await;
    }
    Ok(updated)
}

pub async fn mark_failed(conn: &DatabaseConnection, origin_id: &str) -> Result<(), DbError> {
    if let Some(model) = chat_channel_turn_origin::Entity::find_by_id(origin_id.to_string())
        .one(conn)
        .await?
    {
        let mut active = model.into_active_model();
        active.status = Set("dispatch_failed".to_string());
        active.attempt_count = Set(active.attempt_count.take().unwrap_or_default() + 1);
        active.last_error = Set(Some("prompt_dispatch_failed".to_string()));
        active.updated_at = Set(Utc::now());
        active.update(conn).await?;
    }
    Ok(())
}

pub async fn list_retryable_origins(
    conn: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    conversation_id: i32,
    limit: u64,
) -> Result<Vec<chat_channel_turn_origin::Model>, DbError> {
    use sea_orm::QuerySelect;
    Ok(chat_channel_turn_origin::Entity::find()
        .filter(chat_channel_turn_origin::Column::ChannelId.eq(channel_id))
        .filter(chat_channel_turn_origin::Column::SenderId.eq(sender_id.to_string()))
        .filter(chat_channel_turn_origin::Column::ConversationId.eq(conversation_id))
        .filter(chat_channel_turn_origin::Column::Status.is_in(["queued", "dispatch_failed"]))
        .filter(chat_channel_turn_origin::Column::PromptJson.is_not_null())
        .order_by_asc(chat_channel_turn_origin::Column::CreatedAt)
        .limit(limit)
        .all(conn)
        .await?)
}

pub async fn find_active_by_connection(
    conn: &DatabaseConnection,
    connection_id: &str,
) -> Result<Option<chat_channel_turn_origin::Model>, DbError> {
    Ok(chat_channel_turn_origin::Entity::find()
        .filter(chat_channel_turn_origin::Column::ConnectionId.eq(connection_id.to_string()))
        .filter(chat_channel_turn_origin::Column::Status.eq("dispatched"))
        .order_by_desc(chat_channel_turn_origin::Column::CreatedAt)
        .one(conn)
        .await?)
}

pub async fn find_by_id(
    conn: &DatabaseConnection,
    id: &str,
) -> Result<Option<chat_channel_turn_origin::Model>, DbError> {
    Ok(chat_channel_turn_origin::Entity::find_by_id(id.to_string())
        .one(conn)
        .await?)
}

/// Resolve the durable turn association after prompt dispatch. The turn run is
/// normally created before `mark_dispatched`, but a fast provider/DB schedule
/// can expose the origin first; final capture gets one more authoritative
/// lookup so logs and outbox rows still carry the correct turn_run_id.
pub async fn refresh_turn_run(
    conn: &DatabaseConnection,
    model: chat_channel_turn_origin::Model,
) -> Result<chat_channel_turn_origin::Model, DbError> {
    if model.turn_run_id.is_some() {
        return Ok(model);
    }
    let turn_run_id = conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConversationId.eq(model.conversation_id))
        .filter(conversation_turn_run::Column::ClientMessageId.eq(model.client_message_id.clone()))
        .order_by_desc(conversation_turn_run::Column::StartedAt)
        .one(conn)
        .await?
        .map(|run| run.id);
    let Some(turn_run_id) = turn_run_id else {
        return Ok(model);
    };
    let mut active = model.into_active_model();
    active.turn_run_id = Set(Some(turn_run_id));
    active.updated_at = Set(Utc::now());
    Ok(active.update(conn).await?)
}

pub async fn capture_chunks(
    conn: &DatabaseConnection,
    origin: &chat_channel_turn_origin::Model,
    message_kind: &str,
    final_result_id: &str,
    chunks: &[String],
) -> Result<Vec<chat_channel_outbox::Model>, DbError> {
    let txn = conn.begin().await?;
    let now = Utc::now();
    let mut rows = Vec::with_capacity(chunks.len());
    for (index, content) in chunks.iter().enumerate() {
        if let Some(existing) = chat_channel_outbox::Entity::find()
            .filter(chat_channel_outbox::Column::FinalResultId.eq(final_result_id.to_string()))
            .filter(chat_channel_outbox::Column::ChunkIndex.eq(index as i32))
            .one(&txn)
            .await?
        {
            rows.push(existing);
            continue;
        }
        let row = chat_channel_outbox::ActiveModel {
            id: Set(format!("{final_result_id}-{:04}", index + 1)),
            origin_id: Set(origin.id.clone()),
            channel_id: Set(origin.channel_id),
            sender_id: Set(origin.sender_id.clone()),
            conversation_id: Set(origin.conversation_id),
            origin_message_id: Set(origin.origin_message_id.clone()),
            turn_run_id: Set(origin.turn_run_id.clone()),
            message_kind: Set(message_kind.to_string()),
            final_result_id: Set(final_result_id.to_string()),
            content: Set(content.clone()),
            chunk_index: Set(index as i32),
            chunk_count: Set(chunks.len() as i32),
            status: Set("pending".to_string()),
            attempt_count: Set(0),
            last_error: Set(None),
            context_token_generation: Set(None),
            created_at: Set(now),
            delivered_at: Set(None),
        }
        .insert(&txn)
        .await?;
        rows.push(row);
    }
    if matches!(message_kind, "final" | "terminal_error" | "cancelled") {
        let mut active = origin.clone().into_active_model();
        active.status = Set("final_captured".to_string());
        active.final_captured_at = Set(Some(now));
        active.updated_at = Set(now);
        active.update(&txn).await?;
    }
    txn.commit().await?;
    Ok(rows)
}

pub async fn list_pending_for_sender(
    conn: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    limit: u64,
) -> Result<Vec<chat_channel_outbox::Model>, DbError> {
    use sea_orm::QuerySelect;
    Ok(chat_channel_outbox::Entity::find()
        .filter(chat_channel_outbox::Column::ChannelId.eq(channel_id))
        .filter(chat_channel_outbox::Column::SenderId.eq(sender_id.to_string()))
        .filter(chat_channel_outbox::Column::Status.ne("delivered"))
        .order_by_asc(chat_channel_outbox::Column::CreatedAt)
        .order_by_asc(chat_channel_outbox::Column::ChunkIndex)
        .limit(limit)
        .all(conn)
        .await?)
}

pub async fn mark_chunk_delivered(
    conn: &DatabaseConnection,
    row: chat_channel_outbox::Model,
    generation: Option<u64>,
) -> Result<(), DbError> {
    let mut active = row.into_active_model();
    active.status = Set("delivered".to_string());
    active.attempt_count = Set(active.attempt_count.take().unwrap_or_default() + 1);
    active.last_error = Set(None);
    active.context_token_generation = Set(generation.map(|value| value as i64));
    active.delivered_at = Set(Some(Utc::now()));
    active.update(conn).await?;
    Ok(())
}

pub async fn mark_chunk_deferred(
    conn: &DatabaseConnection,
    row: chat_channel_outbox::Model,
    generation: Option<u64>,
    error: &str,
) -> Result<(), DbError> {
    let mut active = row.into_active_model();
    active.status = Set("pending".to_string());
    active.attempt_count = Set(active.attempt_count.take().unwrap_or_default() + 1);
    active.last_error = Set(Some(error.to_string()));
    active.context_token_generation = Set(generation.map(|value| value as i64));
    active.update(conn).await?;
    Ok(())
}

pub async fn all_chunks_delivered(
    conn: &DatabaseConnection,
    final_result_id: &str,
) -> Result<bool, DbError> {
    let rows = chat_channel_outbox::Entity::find()
        .filter(chat_channel_outbox::Column::FinalResultId.eq(final_result_id.to_string()))
        .all(conn)
        .await?;
    Ok(!rows.is_empty() && rows.iter().all(|row| row.status == "delivered"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::service::chat_channel_service;
    use crate::db::test_helpers::{
        fresh_disk_db, fresh_in_memory_db, seed_conversation, seed_folder,
    };
    use crate::models::agent::AgentType;
    use sea_orm::Database;

    async fn seed_origin() -> (crate::db::AppDatabase, chat_channel_turn_origin::Model) {
        let db = fresh_in_memory_db().await;
        let channel = chat_channel_service::create(
            &db.conn,
            "Weixin".into(),
            "weixin".into(),
            r#"{"base_url":"https://example.invalid","push_mode":"final_and_interactions"}"#.into(),
            true,
            false,
            None,
        )
        .await
        .unwrap();
        let folder_id = seed_folder(&db, "/tmp/codeg-outbox-test").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let origin = register_origin(
            &db.conn,
            NewTurnOrigin {
                channel_id: channel.id,
                sender_id: "wx-user".into(),
                conversation_id,
                connection_id: Some("conn-1".into()),
                origin_message_id: "message_id:42".into(),
                client_message_id: "chat-1-test".into(),
                prompt_json: None,
                target_json: serde_json::json!({
                    "channel_id": channel.id,
                    "chat_id": null,
                    "thread_key": null,
                    "thread_kind": null,
                    "provider_payload": {"weixin_sender_id": "wx-user"}
                })
                .to_string(),
            },
        )
        .await
        .unwrap();
        (db, origin)
    }

    #[tokio::test]
    async fn final_chunks_are_durable_idempotent_and_resume_after_partial_delivery() {
        let (db, origin) = seed_origin().await;
        let chunks = vec![
            "(1/3) first".into(),
            "(2/3) second".into(),
            "(3/3) third".into(),
        ];
        let first = capture_chunks(&db.conn, &origin, "final", "final-1", &chunks)
            .await
            .unwrap();
        let second = capture_chunks(&db.conn, &origin, "final", "final-1", &chunks)
            .await
            .unwrap();
        assert_eq!(first.len(), 3);
        assert_eq!(second.len(), 3);

        mark_chunk_delivered(&db.conn, first[0].clone(), Some(1))
            .await
            .unwrap();
        let pending = list_pending_for_sender(&db.conn, origin.channel_id, "wx-user", 20)
            .await
            .unwrap();
        assert_eq!(pending.len(), 2, "already delivered chunks must not replay");
        assert_eq!(pending[0].chunk_index, 1);
        assert_eq!(pending[1].chunk_index, 2);
        assert!(!all_chunks_delivered(&db.conn, "final-1").await.unwrap());

        for row in pending {
            mark_chunk_delivered(&db.conn, row, Some(2)).await.unwrap();
        }
        assert!(all_chunks_delivered(&db.conn, "final-1").await.unwrap());
    }

    #[tokio::test]
    async fn duplicate_inbound_origin_reuses_one_route() {
        let (db, origin) = seed_origin().await;
        let duplicate = register_origin(
            &db.conn,
            NewTurnOrigin {
                channel_id: origin.channel_id,
                sender_id: origin.sender_id.clone(),
                conversation_id: origin.conversation_id,
                connection_id: Some("another-connection".into()),
                origin_message_id: origin.origin_message_id.clone(),
                client_message_id: "different-client-id".into(),
                target_json: origin.target_json.clone(),
                prompt_json: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(origin.id, duplicate.id);
        assert_eq!(origin.client_message_id, duplicate.client_message_id);
    }

    #[tokio::test]
    async fn pending_final_survives_database_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(dir.path()).await;
        let channel = chat_channel_service::create(
            &db.conn,
            "Weixin".into(),
            "weixin".into(),
            r#"{"push_mode":"final_and_interactions"}"#.into(),
            true,
            false,
            None,
        )
        .await
        .unwrap();
        let folder_id = seed_folder(&db, "/tmp/codeg-outbox-restart").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let origin = register_origin(
            &db.conn,
            NewTurnOrigin {
                channel_id: channel.id,
                sender_id: "wx-user".into(),
                conversation_id,
                connection_id: Some("conn".into()),
                origin_message_id: "message_id:restart".into(),
                client_message_id: "chat-restart".into(),
                prompt_json: None,
                target_json: serde_json::json!({
                    "channel_id": channel.id,
                    "provider_payload": {"weixin_sender_id": "wx-user"}
                })
                .to_string(),
            },
        )
        .await
        .unwrap();
        capture_chunks(
            &db.conn,
            &origin,
            "final",
            "final-restart",
            &["durable answer".into()],
        )
        .await
        .unwrap();
        db.conn.close().await.unwrap();

        let url = format!("sqlite:{}?mode=rw", dir.path().join("source.db").display());
        let reopened = Database::connect(url).await.unwrap();
        let pending = list_pending_for_sender(&reopened, channel.id, "wx-user", 20)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].content, "durable answer");
        reopened.close().await.unwrap();
    }

    #[tokio::test]
    async fn queued_mixed_prompt_survives_database_reopen_exactly_once() {
        use crate::acp::types::PromptInputBlock;

        let dir = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(dir.path()).await;
        let channel = chat_channel_service::create(
            &db.conn,
            "Weixin".into(),
            "weixin".into(),
            r#"{"push_mode":"final_and_interactions"}"#.into(),
            true,
            false,
            None,
        )
        .await
        .unwrap();
        let folder_id = seed_folder(&db, "/tmp/codeg-inbound-restart").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let blocks = vec![
            PromptInputBlock::Text {
                text: "请看附件".into(),
            },
            PromptInputBlock::ResourceLink {
                uri: "file:///tmp/codeg_uploads/chat-channel/weixin/m1/report.pdf".into(),
                name: "report.pdf".into(),
                mime_type: Some("application/pdf".into()),
                description: Some("Weixin inbound attachment".into()),
            },
        ];
        let prompt_json = serde_json::to_string(&blocks).unwrap();
        let first = register_origin(
            &db.conn,
            NewTurnOrigin {
                channel_id: channel.id,
                sender_id: "wx-user".into(),
                conversation_id,
                connection_id: None,
                origin_message_id: "message_id:mixed-restart".into(),
                client_message_id: "chat-mixed-restart".into(),
                prompt_json: Some(prompt_json.clone()),
                target_json: "{}".into(),
            },
        )
        .await
        .unwrap();
        // A provider retry reuses the same durable origin rather than creating
        // a second executable turn.
        let duplicate = register_origin(
            &db.conn,
            NewTurnOrigin {
                channel_id: channel.id,
                sender_id: "wx-user".into(),
                conversation_id,
                connection_id: None,
                origin_message_id: "message_id:mixed-restart".into(),
                client_message_id: "ignored-duplicate".into(),
                prompt_json: Some(prompt_json),
                target_json: "{}".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(first.id, duplicate.id);
        db.conn.close().await.unwrap();

        let url = format!("sqlite:{}?mode=rw", dir.path().join("source.db").display());
        let reopened = Database::connect(url).await.unwrap();
        let queued = list_retryable_origins(&reopened, channel.id, "wx-user", conversation_id, 10)
            .await
            .unwrap();
        assert_eq!(queued.len(), 1);
        let restored: Vec<PromptInputBlock> =
            serde_json::from_str(queued[0].prompt_json.as_deref().unwrap()).unwrap();
        assert_eq!(restored, blocks);
        reopened.close().await.unwrap();
    }
}
