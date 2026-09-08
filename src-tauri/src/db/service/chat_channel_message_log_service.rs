use chrono::Utc;
use sea_orm::prelude::DateTimeUtc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, Set,
};

use crate::db::entities::chat_channel_message_log;
use crate::db::error::DbError;

pub async fn create_log(
    conn: &DatabaseConnection,
    channel_id: i32,
    direction: &str,
    message_type: &str,
    content_preview: &str,
    status: &str,
    error_detail: Option<String>,
) -> Result<(), DbError> {
    create_correlated_log(
        conn,
        channel_id,
        direction,
        message_type,
        content_preview,
        status,
        error_detail,
        None,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn create_correlated_log(
    conn: &DatabaseConnection,
    channel_id: i32,
    direction: &str,
    message_type: &str,
    content_preview: &str,
    status: &str,
    error_detail: Option<String>,
    origin_message_id: Option<&str>,
    turn_run_id: Option<&str>,
    final_result_id: Option<&str>,
) -> Result<(), DbError> {
    let content_length = content_preview.chars().count().min(i32::MAX as usize) as i32;
    // Correlated turn/outbox logs are operational telemetry. Keep their size
    // and IDs, but never duplicate user prompts or final answers into a second
    // plaintext store where credentials could appear in a preview.
    let stored_preview = if origin_message_id.is_some() && !content_preview.is_empty() {
        format!("[{content_length} chars]")
    } else {
        truncate_preview(content_preview)
    };
    let active = chat_channel_message_log::ActiveModel {
        id: NotSet,
        channel_id: Set(channel_id),
        direction: Set(direction.to_string()),
        message_type: Set(message_type.to_string()),
        content_preview: Set(stored_preview),
        status: Set(status.to_string()),
        error_detail: Set(error_detail),
        origin_message_id: Set(origin_message_id.map(str::to_string)),
        turn_run_id: Set(turn_run_id.map(str::to_string)),
        final_result_id: Set(final_result_id.map(str::to_string)),
        content_length: Set(Some(content_length)),
        created_at: Set(Utc::now()),
    };
    active.insert(conn).await?;
    Ok(())
}

pub async fn list_by_channel(
    conn: &DatabaseConnection,
    channel_id: i32,
    limit: u64,
    offset: u64,
) -> Result<Vec<chat_channel_message_log::Model>, DbError> {
    use sea_orm::PaginatorTrait;
    Ok(chat_channel_message_log::Entity::find()
        .filter(chat_channel_message_log::Column::ChannelId.eq(channel_id))
        .order_by_desc(chat_channel_message_log::Column::CreatedAt)
        .paginate(conn, limit)
        .fetch_page(offset / limit)
        .await?)
}

pub async fn cleanup_old_logs(
    conn: &DatabaseConnection,
    older_than: DateTimeUtc,
) -> Result<u64, DbError> {
    let result = chat_channel_message_log::Entity::delete_many()
        .filter(chat_channel_message_log::Column::CreatedAt.lt(older_than))
        .exec(conn)
        .await?;
    Ok(result.rows_affected)
}

fn truncate_preview(s: &str) -> String {
    if s.len() <= 200 {
        s.to_string()
    } else {
        let mut end = 200;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}
