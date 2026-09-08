use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, Set,
};

use crate::db::entities::chat_channel_sender_context;
use crate::db::error::DbError;

/// Read a sender route without manufacturing an empty row. Ordinary chat
/// messages use this to distinguish "this sender has never selected a
/// conversation" from a temporary database/restore failure.
pub async fn find(
    conn: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
) -> Result<Option<chat_channel_sender_context::Model>, DbError> {
    Ok(chat_channel_sender_context::Entity::find()
        .filter(chat_channel_sender_context::Column::ChannelId.eq(channel_id))
        .filter(chat_channel_sender_context::Column::SenderId.eq(sender_id))
        .one(conn)
        .await?)
}

pub async fn get_or_create(
    conn: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
) -> Result<chat_channel_sender_context::Model, DbError> {
    let existing = find(conn, channel_id, sender_id).await?;

    if let Some(model) = existing {
        return Ok(model);
    }

    let now = Utc::now();
    let active = chat_channel_sender_context::ActiveModel {
        id: NotSet,
        channel_id: Set(channel_id),
        sender_id: Set(sender_id.to_string()),
        current_folder_id: Set(None),
        current_agent_type: Set(None),
        current_conversation_id: Set(None),
        current_connection_id: Set(None),
        auto_approve: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    };
    Ok(active.insert(conn).await?)
}

pub async fn update_folder(
    conn: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    folder_id: Option<i32>,
) -> Result<chat_channel_sender_context::Model, DbError> {
    let model = get_or_create(conn, channel_id, sender_id).await?;
    let mut active = model.into_active_model();
    active.current_folder_id = Set(folder_id);
    active.updated_at = Set(Utc::now());
    Ok(active.update(conn).await?)
}

pub async fn update_agent(
    conn: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    agent_type: Option<String>,
) -> Result<chat_channel_sender_context::Model, DbError> {
    let model = get_or_create(conn, channel_id, sender_id).await?;
    let mut active = model.into_active_model();
    active.current_agent_type = Set(agent_type);
    active.updated_at = Set(Utc::now());
    Ok(active.update(conn).await?)
}

pub async fn update_session(
    conn: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    conversation_id: Option<i32>,
    connection_id: Option<String>,
) -> Result<chat_channel_sender_context::Model, DbError> {
    let model = get_or_create(conn, channel_id, sender_id).await?;
    let mut active = model.into_active_model();
    active.current_conversation_id = Set(conversation_id);
    active.current_connection_id = Set(connection_id);
    active.updated_at = Set(Utc::now());
    Ok(active.update(conn).await?)
}

/// Atomically select a durable conversation route while leaving the process-
/// local ACP connection empty. Lazy restore fills the connection id only after
/// ACP has actually become usable.
pub async fn update_binding(
    conn: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    folder_id: i32,
    agent_type: String,
    conversation_id: i32,
) -> Result<chat_channel_sender_context::Model, DbError> {
    let model = get_or_create(conn, channel_id, sender_id).await?;
    let mut active = model.into_active_model();
    active.current_folder_id = Set(Some(folder_id));
    active.current_agent_type = Set(Some(agent_type));
    active.current_conversation_id = Set(Some(conversation_id));
    active.current_connection_id = Set(None);
    active.updated_at = Set(Utc::now());
    Ok(active.update(conn).await?)
}

pub async fn clear_session(
    conn: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
) -> Result<chat_channel_sender_context::Model, DbError> {
    update_session(conn, channel_id, sender_id, None, None).await
}

/// Forget only the ephemeral in-process ACP binding while retaining the
/// durable conversation selection. Server restart and reconnect recovery use
/// that conversation id to rebuild `SessionBridge` on the next message.
pub async fn clear_connection(
    conn: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
) -> Result<chat_channel_sender_context::Model, DbError> {
    let model = get_or_create(conn, channel_id, sender_id).await?;
    let mut active = model.into_active_model();
    active.current_connection_id = Set(None);
    active.updated_at = Set(Utc::now());
    let updated = active.update(conn).await?;
    let sender_key = crate::chat_channel::types::sender_log_key(sender_id);
    tracing::info!(
        stage = "transient_connection_cleared",
        channel_id,
        sender_key,
        conversation_id = ?updated.current_conversation_id,
        "[ChatChannel] transient ACP connection cleared"
    );
    tracing::info!(
        stage = "durable_binding_preserved",
        channel_id,
        sender_key,
        conversation_id = ?updated.current_conversation_id,
        "[ChatChannel] durable conversation binding preserved"
    );
    Ok(updated)
}

pub async fn update_auto_approve(
    conn: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    auto_approve: bool,
) -> Result<chat_channel_sender_context::Model, DbError> {
    let model = get_or_create(conn, channel_id, sender_id).await?;
    let mut active = model.into_active_model();
    active.auto_approve = Set(auto_approve);
    active.updated_at = Set(Utc::now());
    Ok(active.update(conn).await?)
}
