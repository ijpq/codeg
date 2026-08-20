use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::codex_quota::codex_quota_snapshot_core;
use crate::models::CodexQuotaSnapshot;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexQuotaSnapshotParams {
    pub conversation_id: Option<i32>,
}

pub async fn codex_quota_snapshot(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CodexQuotaSnapshotParams>,
) -> Result<Json<Option<CodexQuotaSnapshot>>, AppCommandError> {
    Ok(Json(
        codex_quota_snapshot_core(&state.db.conn, params.conversation_id).await?,
    ))
}
