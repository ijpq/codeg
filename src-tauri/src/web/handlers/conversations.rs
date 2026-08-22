use std::sync::Arc;

use axum::{extract::Extension, response::IntoResponse, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::conversation_branches as branch_commands;
use crate::commands::conversations as conv_commands;
use crate::models::*;

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
    /// Optional turn-window selectors (mutually exclusive). Absent = legacy
    /// full response.
    #[serde(default)]
    pub tail_turns: Option<usize>,
    #[serde(default)]
    pub from_index: Option<usize>,
    #[serde(default)]
    pub before_cursor: Option<String>,
    #[serde(default)]
    pub user_turn_limit: Option<u32>,
}

pub async fn get_folder_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<GetFolderConversationParams>,
) -> Result<axum::response::Response, AppCommandError> {
    let started = std::time::Instant::now();
    let db = &state.db;
    let cursor_requested = params.before_cursor.is_some() || params.user_turn_limit.is_some();
    let result = if cursor_requested {
        conv_commands::get_folder_conversation_page_with_live_core(
            &db.conn,
            &state.connection_manager,
            &state.chat_channel_manager,
            &state.emitter,
            params.conversation_id,
            conv_commands::ConversationHistoryRequest {
                before_cursor: params.before_cursor,
                user_turn_limit: params
                    .user_turn_limit
                    .unwrap_or(conv_commands::DEFAULT_HISTORY_PAGE_USER_TURNS),
            },
        )
        .await?
    } else {
        let window = conv_commands::resolve_turn_window_req(params.tail_turns, params.from_index)?;
        conv_commands::get_folder_conversation_with_live_core(
            &db.conn,
            &state.connection_manager,
            &state.chat_channel_manager,
            &state.emitter,
            params.conversation_id,
            window,
        )
        .await?
    };
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
        conversation_id = params.conversation_id,
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
    Ok(([("content-type", "application/json")], response_bytes).into_response())
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
    Ok(Json(
        branch_commands::create_conversation_branch_core(
            &state.db,
            &state.connection_manager,
            &state.emitter,
            &state.data_dir,
            "web:conversation-branch".into(),
            params.request,
        )
        .await?,
    ))
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
    pub summary: String,
    #[serde(default)]
    pub deliverable_ids: Vec<String>,
}

pub async fn merge_conversation_branch(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<MergeConversationBranchParams>,
) -> Result<Json<crate::db::service::conversation_branch_service::MergeBranchResult>, AppCommandError>
{
    Ok(Json(
        branch_commands::merge_conversation_branch_core(
            &state.db,
            &state.emitter,
            params.branch_conversation_id,
            params.request_id,
            params.summary,
            params.deliverable_ids,
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
