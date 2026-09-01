use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::{extract::Extension, response::IntoResponse, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::conversation_branches as branch_commands;
use crate::commands::conversations as conv_commands;
use crate::models::*;

struct HistoryRequestCancellation {
    cancelled: Arc<AtomicBool>,
    completed: bool,
}

impl Drop for HistoryRequestCancellation {
    fn drop(&mut self) {
        if !self.completed {
            self.cancelled.store(true, Ordering::Relaxed);
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListAllConversationsParams {
    pub folder_ids: Option<Vec<i32>>,
    pub agent_type: Option<AgentType>,
    pub search: Option<String>,
    pub sort_by: Option<String>,
    pub status: Option<String>,
    pub include_children: Option<bool>,
}

pub async fn list_all_conversations(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ListAllConversationsParams>,
) -> Result<Json<Vec<DbConversationSummary>>, AppCommandError> {
    Ok(Json(
        conv_commands::list_all_conversations_core(
            &state.db.conn,
            &state.emitter,
            &state.chat_channel_manager,
            conv_commands::ListAllConversationsOptions {
                folder_ids: params.folder_ids,
                agent_type: params.agent_type,
                search: params.search,
                sort_by: params.sort_by,
                status: params.status,
                include_children: params.include_children.unwrap_or(false),
            },
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListChildConversationsParams {
    pub parent_conversation_id: i32,
}

pub async fn list_child_conversations(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ListChildConversationsParams>,
) -> Result<Json<Vec<DbConversationSummary>>, AppCommandError> {
    Ok(Json(
        conv_commands::list_child_conversations_core(&state.db.conn, params.parent_conversation_id)
            .await?,
    ))
}

pub async fn list_opened_tabs(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<OpenedTabsSnapshot>, AppCommandError> {
    Ok(Json(
        conv_commands::list_opened_tabs_core(&state.db.conn).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOpenedTabsParams {
    pub items: Vec<OpenedTab>,
    pub expected_version: i64,
    pub origin: String,
}

pub async fn save_opened_tabs(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<SaveOpenedTabsParams>,
) -> Result<Json<SaveTabsOutcome>, AppCommandError> {
    Ok(Json(
        conv_commands::save_opened_tabs_core(
            &state.db.conn,
            &state.emitter,
            params.items,
            params.expected_version,
            params.origin,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListConversationsParams {
    pub agent_type: Option<AgentType>,
    pub search: Option<String>,
    pub sort_by: Option<String>,
    pub folder_path: Option<String>,
}

pub async fn list_conversations(
    Json(params): Json<ListConversationsParams>,
) -> Result<Json<Vec<ConversationSummary>>, AppCommandError> {
    let result = conv_commands::list_conversations(
        params.agent_type,
        params.search,
        params.sort_by,
        params.folder_path,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetConversationParams {
    pub agent_type: AgentType,
    pub conversation_id: String,
}

pub async fn get_conversation(
    Json(params): Json<GetConversationParams>,
) -> Result<Json<ConversationDetail>, AppCommandError> {
    let result = conv_commands::get_conversation(params.agent_type, params.conversation_id).await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetFolderConversationParams {
    pub conversation_id: i32,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub request_generation: Option<u64>,
    /// Optional legacy turn-window selectors (mutually exclusive). New callers
    /// use the opaque history cursor. If every selector is absent we still
    /// return the bounded newest page; ordinary UI opens must never mean
    /// "parse the complete transcript".
    #[serde(default)]
    pub tail_turns: Option<usize>,
    #[serde(default)]
    pub from_index: Option<usize>,
    #[serde(default)]
    pub before_cursor: Option<String>,
    #[serde(default)]
    pub user_turn_limit: Option<u32>,
}

fn legacy_turn_window_requested(params: &GetFolderConversationParams) -> bool {
    params.tail_turns.is_some() || params.from_index.is_some()
}

pub async fn get_folder_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<GetFolderConversationParams>,
) -> Result<axum::response::Response, AppCommandError> {
    let started = std::time::Instant::now();
    let db = &state.db;
    if params.tail_turns.is_some() && params.from_index.is_some() {
        return Err(AppCommandError::invalid_input(
            "tailTurns and fromIndex are mutually exclusive",
        ));
    }
    if params.from_index.is_some() {
        return Err(AppCommandError::invalid_input(
            "fromIndex is retired for conversation opens; use beforeCursor",
        ));
    }
    if legacy_turn_window_requested(&params) {
        tracing::warn!(
            conversation_id = params.conversation_id,
            request_id = params.request_id.as_deref(),
            "[conversation][perf] deprecated tailTurns was converted to bounded cursor history"
        );
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut cancellation_guard = HistoryRequestCancellation {
        cancelled: cancelled.clone(),
        completed: false,
    };
    let result = conv_commands::get_folder_conversation_page_with_live_core(
        &db.conn,
        &state.connection_manager,
        &state.chat_channel_manager,
        &state.emitter,
        params.conversation_id,
        conv_commands::ConversationHistoryRequest {
            before_cursor: params.before_cursor.clone(),
            user_turn_limit: params
                .user_turn_limit
                .unwrap_or(conv_commands::DEFAULT_HISTORY_PAGE_USER_TURNS),
            cancellation: Some(cancelled),
        },
    )
    .await?;
    let data_read_elapsed_ms = started.elapsed().as_millis() as u64;
    let serialization_started = std::time::Instant::now();
    let response_bytes = serde_json::to_vec(&result).map_err(|error| {
        AppCommandError::task_execution_failed("Failed to serialize conversation detail")
            .with_detail(error.to_string())
    })?;
    let uncompressed_bytes = response_bytes.len();
    let serialization_elapsed_ms = serialization_started.elapsed().as_millis() as u64;
    tracing::info!(
        route = "/api/get_folder_conversation",
        request_id = params.request_id.as_deref(),
        request_generation = params.request_generation,
        conversation_id = params.conversation_id,
        source_conversation_id = result
            .branch_history
            .as_ref()
            .map(|history| history.source_conversation_id),
        cursor = params.before_cursor.as_deref(),
        page_size = params.user_turn_limit,
        bounded_history = result.history_page.is_some() || result.turns_offset.is_some(),
        loaded_turns = result.turns.len(),
        has_more = result
            .history_page
            .as_ref()
            .is_some_and(|page| page.has_more),
        data_read_elapsed_ms,
        serialization_elapsed_ms,
        uncompressed_bytes,
        "[HTTP][perf] conversation detail prepared"
    );
    cancellation_guard.completed = true;
    Ok(([("content-type", "application/json")], response_bytes).into_response())
}

#[derive(Deserialize)]
pub struct GetDeferredHistoryContentParams {
    pub reference: String,
}

pub async fn get_deferred_history_content(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<GetDeferredHistoryContentParams>,
) -> Result<Json<conv_commands::DeferredHistoryContent>, AppCommandError> {
    Ok(Json(
        conv_commands::get_deferred_history_content_core(&state.db.conn, params.reference).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListConversationBranchMergesParams {
    pub conversation_id: i32,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub limit: Option<u64>,
}

pub async fn list_conversation_branch_merges(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ListConversationBranchMergesParams>,
) -> Result<
    Json<crate::db::service::conversation_branch_service::ConversationBranchMergePreviewPage>,
    AppCommandError,
> {
    Ok(Json(
        conv_commands::list_conversation_branch_merges_core(
            &state.db.conn,
            params.conversation_id,
            params.offset.unwrap_or(0),
            params.limit.unwrap_or(20),
        )
        .await?,
    ))
}

pub async fn list_conversation_branches(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ListConversationBranchMergesParams>,
) -> Result<
    Json<crate::db::service::conversation_branch_service::ConversationSourceBranchPreviewPage>,
    AppCommandError,
> {
    Ok(Json(
        conv_commands::list_conversation_branches_core(
            &state.db.conn,
            params.conversation_id,
            params.offset.unwrap_or(0),
            params.limit.unwrap_or(20),
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListConversationOutputWindowParams {
    pub conversation_id: i32,
    pub turn_refs: Vec<conv_commands::VisibleConversationTurnRef>,
}

pub async fn list_conversation_output_window(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ListConversationOutputWindowParams>,
) -> Result<Json<conv_commands::ConversationOutputWindow>, AppCommandError> {
    Ok(Json(
        conv_commands::list_conversation_output_window_core(
            &state.db.conn,
            params.conversation_id,
            params.turn_refs,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnoseCodexRolloutSizeParams {
    pub conversation_id: i32,
}

pub async fn diagnose_codex_rollout_size(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<DiagnoseCodexRolloutSizeParams>,
) -> Result<Json<crate::parsers::codex::CodexRolloutSizeDiagnostics>, AppCommandError> {
    Ok(Json(
        conv_commands::diagnose_codex_rollout_size_core(&state.db.conn, params.conversation_id)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetFolderConversationTurnsParams {
    pub conversation_id: i32,
    pub before_index: usize,
    pub limit: usize,
}

pub async fn get_folder_conversation_turns(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<GetFolderConversationTurnsParams>,
) -> Result<Json<ConversationTurnsPage>, AppCommandError> {
    let result = conv_commands::get_folder_conversation_turns_core(
        &state.db.conn,
        params.conversation_id,
        params.before_index,
        params.limit,
    )
    .await?;
    Ok(Json(result))
}

pub async fn list_folders() -> Result<Json<Vec<FolderInfo>>, AppCommandError> {
    let result = conv_commands::list_folders().await?;
    Ok(Json(result))
}

pub async fn get_stats() -> Result<Json<AgentStats>, AppCommandError> {
    let result = conv_commands::get_stats().await?;
    Ok(Json(result))
}

pub async fn get_sidebar_data() -> Result<Json<SidebarData>, AppCommandError> {
    let result = conv_commands::get_sidebar_data().await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportLocalConversationsParams {
    pub folder_id: i32,
}

pub async fn import_local_conversations(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ImportLocalConversationsParams>,
) -> Result<Json<ImportResult>, AppCommandError> {
    Ok(Json(
        conv_commands::import_local_conversations_core(
            &state.db.conn,
            &state.emitter,
            &state.chat_channel_manager,
            params.folder_id,
        )
        .await?,
    ))
}

pub async fn scan_importable_sessions(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<ScanResult>, AppCommandError> {
    Ok(Json(
        conv_commands::scan_importable_sessions_core(
            &state.db.conn,
            &state.emitter,
            &state.chat_channel_manager,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSelectedSessionsParams {
    pub selections: Vec<SelectedSessionKey>,
}

pub async fn import_selected_sessions(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ImportSelectedSessionsParams>,
) -> Result<Json<ImportSelectedResult>, AppCommandError> {
    Ok(Json(
        conv_commands::import_selected_sessions_core(
            &state.db.conn,
            &state.emitter,
            params.selections,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversationParams {
    pub folder_id: i32,
    pub agent_type: AgentType,
    pub title: Option<String>,
}

pub async fn create_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CreateConversationParams>,
) -> Result<Json<i32>, AppCommandError> {
    let db = &state.db;
    let result = conv_commands::create_conversation_core(
        &db.conn,
        params.folder_id,
        params.agent_type,
        params.title,
    )
    .await?;
    conv_commands::emit_conversation_upsert(&state.emitter, &db.conn, result).await;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct CreateConversationBranchParams {
    pub request: branch_commands::CreateConversationBranchRequest,
}

pub async fn create_conversation_branch(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CreateConversationBranchParams>,
) -> Result<Json<branch_commands::CreateConversationBranchResult>, AppCommandError> {
    // A native fork can take tens of seconds. Keep the transaction-shaped
    // operation alive if the browser reloads or closes the response stream;
    // its stable request id lets the reloaded queue retrieve the same result.
    let db = crate::db::AppDatabase {
        conn: state.db.conn.clone(),
    };
    let manager = state.connection_manager.clone_ref();
    let emitter = state.emitter.clone();
    let data_dir = state.data_dir.clone();
    let result = tokio::spawn(async move {
        branch_commands::create_conversation_branch_core(
            &db,
            &manager,
            &emitter,
            &data_dir,
            "web:conversation-branch".into(),
            params.request,
        )
        .await
    })
    .await
    .map_err(|error| {
        AppCommandError::task_execution_failed(format!(
            "Branch creation task stopped unexpectedly: {error}"
        ))
    })??;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetConversationBranchInfoParams {
    pub conversation_id: i32,
}

pub async fn get_conversation_branch_info(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<GetConversationBranchInfoParams>,
) -> Result<
    Json<Option<crate::db::service::conversation_branch_service::ConversationBranchInfo>>,
    AppCommandError,
> {
    Ok(Json(
        branch_commands::get_conversation_branch_info_core(&state.db, params.conversation_id)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeConversationBranchParams {
    pub branch_conversation_id: i32,
    pub request_id: String,
}

pub async fn merge_conversation_branch(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<MergeConversationBranchParams>,
) -> Result<Json<crate::db::service::conversation_branch_service::MergeBranchResult>, AppCommandError>
{
    Ok(Json(
        branch_commands::merge_conversation_branch_core(
            &state.db,
            &state.connection_manager,
            &state.emitter,
            params.branch_conversation_id,
            params.request_id,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateChatConversationParams {
    pub agent_type: AgentType,
    pub title: Option<String>,
    /// Reuse an eagerly-created scratch dir (from `create_chat_dir`) instead of
    /// minting a new one, so the ACP cwd stays put across the first send.
    pub existing_dir: Option<String>,
}

pub async fn create_chat_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CreateChatConversationParams>,
) -> Result<Json<conv_commands::CreateChatConversationResult>, AppCommandError> {
    let result = conv_commands::create_chat_conversation_core(
        &state.db.conn,
        &state.data_dir,
        params.agent_type,
        params.title,
        params.existing_dir.as_deref(),
    )
    .await?;
    conv_commands::emit_conversation_upsert(&state.emitter, &state.db.conn, result.conversation_id)
        .await;
    Ok(Json(result))
}

/// Eagerly create a chat-mode scratch directory (no DB rows) and return its
/// path. Web twin of the `create_chat_dir` Tauri command — lets the browser
/// client connect ACP at a real cwd the instant "no-folder mode" is selected.
pub async fn create_chat_dir(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<conv_commands::CreateChatDirResult>, AppCommandError> {
    let path = conv_commands::create_chat_dir_core(&state.data_dir)?;
    Ok(Json(conv_commands::CreateChatDirResult { path }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConversationStatusParams {
    pub conversation_id: i32,
    pub status: String,
}

pub async fn update_conversation_status(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateConversationStatusParams>,
) -> Result<Json<()>, AppCommandError> {
    conv_commands::update_conversation_status_core(
        &state.db.conn,
        params.conversation_id,
        params.status,
    )
    .await?;
    conv_commands::emit_conversation_upsert(&state.emitter, &state.db.conn, params.conversation_id)
        .await;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConversationTitleParams {
    pub conversation_id: i32,
    pub title: String,
}

pub async fn update_conversation_title(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateConversationTitleParams>,
) -> Result<Json<()>, AppCommandError> {
    conv_commands::update_conversation_title_core(
        &state.db.conn,
        params.conversation_id,
        params.title,
    )
    .await?;
    conv_commands::emit_conversation_upsert(&state.emitter, &state.db.conn, params.conversation_id)
        .await;
    conv_commands::sync_conversation_title_to_channels_core(
        &state.db.conn,
        &state.chat_channel_manager,
        params.conversation_id,
    )
    .await;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConversationPinnedParams {
    pub conversation_id: i32,
    pub pinned: bool,
}

pub async fn update_conversation_pinned(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateConversationPinnedParams>,
) -> Result<Json<()>, AppCommandError> {
    conv_commands::update_conversation_pinned_core(
        &state.db.conn,
        params.conversation_id,
        params.pinned,
    )
    .await?;
    conv_commands::emit_conversation_upsert(&state.emitter, &state.db.conn, params.conversation_id)
        .await;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConversationParams {
    pub conversation_id: i32,
}

pub async fn delete_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<DeleteConversationParams>,
) -> Result<Json<()>, AppCommandError> {
    conv_commands::delete_conversation_with_cleanup_core(
        &state.emitter,
        &state.db.conn,
        params.conversation_id,
    )
    .await?;
    Ok(Json(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_detail_without_selectors_uses_bounded_cursor_path() {
        let params = GetFolderConversationParams {
            conversation_id: 26,
            request_id: Some("request-1".into()),
            request_generation: Some(7),
            tail_turns: None,
            from_index: None,
            before_cursor: None,
            user_turn_limit: None,
        };
        assert!(!legacy_turn_window_requested(&params));
    }

    #[test]
    fn deprecated_tail_selector_is_detected_for_bounded_conversion() {
        let params = GetFolderConversationParams {
            conversation_id: 26,
            request_id: None,
            request_generation: None,
            tail_turns: Some(10),
            from_index: None,
            before_cursor: None,
            user_turn_limit: None,
        };
        assert!(legacy_turn_window_requested(&params));
    }
}
