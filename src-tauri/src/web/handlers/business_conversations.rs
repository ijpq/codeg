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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryParams {
    #[serde(alias = "conversation_id")]
    pub conversation_id: i32,
    #[serde(alias = "message_id")]
    pub message_id: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum BusinessSubmitParams {
    Direct(crate::db::service::business_delivery_service::SubmitRequest),
    Wrapped {
        request: crate::db::service::business_delivery_service::SubmitRequest,
    },
}
impl BusinessSubmitParams {
    fn into_request(self) -> crate::db::service::business_delivery_service::SubmitRequest {
        match self {
            Self::Direct(request) | Self::Wrapped { request } => request,
        }
    }
}
pub async fn submit_business_message(
    Extension(state): Extension<Arc<AppState>>,
    Json(request): Json<BusinessSubmitParams>,
) -> Result<Json<crate::db::service::business_delivery_service::Delivery>, AppCommandError> {
    crate::commands::business_delivery::submit_core(
        request.into_request(),
        &state.connection_manager,
        &state.db,
        &state.data_dir,
        state.emitter.clone(),
    )
    .await
    .map(Json)
    .map_err(delivery_error)
}
pub async fn get_business_message(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<DeliveryParams>,
) -> Result<Json<crate::db::service::business_delivery_service::Delivery>, AppCommandError> {
    use crate::db::service::business_delivery_service as service;
    let item = service::get(&state.db.conn, params.conversation_id, &params.message_id)
        .await
        .map_err(|e| delivery_error(crate::acp::error::AcpError::protocol(e.to_string())))?;
    if matches!(item.status.as_str(), "delivered" | "running" | "cancelling") {
        service::reconcile(&state.db.conn, &item)
            .await
            .map_err(|e| delivery_error(crate::acp::error::AcpError::protocol(e.to_string())))?;
    }
    service::get(&state.db.conn, params.conversation_id, &params.message_id)
        .await
        .map(Json)
        .map_err(|e| delivery_error(crate::acp::error::AcpError::protocol(e.to_string())))
}
pub async fn cancel_business_message(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<DeliveryParams>,
) -> Result<Json<crate::db::service::business_delivery_service::Delivery>, AppCommandError> {
    crate::commands::business_delivery::cancel_core(
        params.conversation_id,
        &params.message_id,
        &state.connection_manager,
        &state.db,
    )
    .await
    .map(Json)
    .map_err(delivery_error)
}
fn delivery_error(e: crate::acp::error::AcpError) -> AppCommandError {
    use crate::app_error::AppErrorCode;
    let message = e.to_string();
    let code = if message.contains("REQUIRED_AGENT_MISMATCH") {
        AppErrorCode::RequiredAgentMismatch
    } else if message.contains("BUSINESS_SWITCH_PENDING") {
        AppErrorCode::BusinessSwitchPending
    } else if message.contains("IDEMPOTENCY_KEY_REUSED") {
        AppErrorCode::IdempotencyKeyReused
    } else if message.contains("DELIVERY_MODE_UNSUPPORTED") {
        AppErrorCode::DeliveryModeUnsupported
    } else if message.contains("INVALID_BUSINESS_MESSAGE") {
        AppErrorCode::InvalidInput
    } else if message.contains("CONVERSATION_NOT_FOUND")
        || message.contains("BUSINESS_MESSAGE_NOT_FOUND")
    {
        AppErrorCode::NotFound
    } else {
        AppErrorCode::TaskExecutionFailed
    };
    AppCommandError::new(code, message)
}
