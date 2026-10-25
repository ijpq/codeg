use crate::{
    app_error::AppCommandError,
    app_state::AppState,
    commands::business_conversations as commands,
    db::service::{business_conversation_service as business, conversation_service},
};
use axum::{Extension, Json};
use serde::Deserialize;
use std::sync::Arc;
fn error(e: impl std::fmt::Display) -> AppCommandError {
    AppCommandError::task_execution_failed(e.to_string())
}
#[derive(Deserialize)]
pub struct SwitchParams {
    pub request: business::AgentSwitchRequest,
}
pub async fn switch_business_agent(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<SwitchParams>,
) -> Result<Json<business::BusinessConversationState>, AppCommandError> {
    commands::switch_agent_core(
        params.request,
        &state.connection_manager,
        &state.db,
        &state.data_dir,
        state.emitter.clone(),
    )
    .await
    .map(Json)
    .map_err(error)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationParams {
    pub conversation_id: i32,
}
pub async fn get_business_conversation_state(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ConversationParams>,
) -> Result<Json<Option<business::BusinessConversationState>>, AppCommandError> {
    commands::get_state_core(
        params.conversation_id,
        &state.connection_manager,
        &state.db,
        &state.data_dir,
        state.emitter.clone(),
    )
    .await
    .map(Json)
    .map_err(error)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskParams {
    pub conversation_id: i32,
    pub revision: i64,
    pub task: business::BusinessTaskState,
}
pub async fn update_business_task_state(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<TaskParams>,
) -> Result<Json<()>, AppCommandError> {
    business::update_task_state(
        &state.db.conn,
        params.conversation_id,
        params.revision,
        &params.task,
    )
    .await
    .map(Json)
    .map_err(error)
}
pub async fn get_business_shared_memory(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ConversationParams>,
) -> Result<Json<Vec<business::SharedMemory>>, AppCommandError> {
    let summary = conversation_service::get_by_id(&state.db.conn, params.conversation_id)
        .await
        .map_err(error)?;
    business::shared_memory(&state.db.conn, params.conversation_id, summary.folder_id)
        .await
        .map(Json)
        .map_err(error)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryParams {
    pub conversation_id: i32,
    pub item: business::SharedMemory,
}
pub async fn save_business_shared_memory(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<MemoryParams>,
) -> Result<Json<()>, AppCommandError> {
    commands::save_memory_core(&state.db, params.conversation_id, &params.item)
        .await
        .map(Json)
        .map_err(error)
}
