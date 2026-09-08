use sea_orm::DatabaseConnection;

use crate::app_error::AppCommandError;
use crate::db::service::conversation_service;
#[cfg(feature = "tauri-runtime")]
use crate::db::AppDatabase;
use crate::models::{AgentType, CodexQuotaSnapshot};
use crate::parsers::codex_quota::latest_quota_snapshot;

/// Return the last allowance metadata attached to a real Codex response.
///
/// For an existing conversation this is strictly scoped to its external
/// session. Draft composers (no DB id yet) fall back to the most recently
/// active Codex rollout so the badge need not disappear before first send.
pub async fn codex_quota_snapshot_core(
    conn: &DatabaseConnection,
    conversation_id: Option<i32>,
) -> Result<Option<CodexQuotaSnapshot>, AppCommandError> {
    let external_id = if let Some(conversation_id) = conversation_id {
        let conversation = conversation_service::get_by_id(conn, conversation_id)
            .await
            .map_err(AppCommandError::from)?;
        if conversation.agent_type != AgentType::Codex {
            return Ok(None);
        }
        conversation.external_id
    } else {
        None
    };

    tokio::task::spawn_blocking(move || latest_quota_snapshot(external_id.as_deref()))
        .await
        .map_err(|error| {
            AppCommandError::task_execution_failed(format!(
                "Codex quota reader task failed: {error}"
            ))
        })?
        .map_err(|error| AppCommandError::io_error(format!("Codex quota read failed: {error}")))
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn codex_quota_snapshot(
    db: tauri::State<'_, AppDatabase>,
    conversation_id: Option<i32>,
) -> Result<Option<CodexQuotaSnapshot>, AppCommandError> {
    codex_quota_snapshot_core(&db.conn, conversation_id).await
}
