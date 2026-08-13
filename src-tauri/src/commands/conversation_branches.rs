use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::acp::error::AcpError;
use crate::acp::manager::ConnectionManager;
use crate::app_error::AppCommandError;
use crate::commands::acp::build_session_runtime_env;
use crate::commands::conversations::emit_conversation_upsert;
use crate::db::service::{conversation_branch_service, conversation_service, folder_service};
use crate::db::AppDatabase;
use crate::web::event_bridge::EventEmitter;
#[cfg(feature = "tauri-runtime")]
use tauri::Manager;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversationBranchRequest {
    pub source_conversation_id: i32,
    pub fork_message_id: Option<String>,
    pub snapshot_context: Option<String>,
    pub preferred_mode_id: Option<String>,
    #[serde(default)]
    pub preferred_config_values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversationBranchResult {
    pub branch_conversation_id: i32,
    pub source_conversation_id: i32,
    pub folder_id: i32,
    pub connection_id: Option<String>,
    pub fork_mode: String,
    pub fallback_reason: Option<String>,
}

pub async fn create_conversation_branch_core(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    data_dir: &Path,
    owner_label: String,
    request: CreateConversationBranchRequest,
) -> Result<CreateConversationBranchResult, AppCommandError> {
    let source = conversation_service::get_by_id(&db.conn, request.source_conversation_id)
        .await
        .map_err(AppCommandError::from)?;
    let folder = folder_service::get_folder_by_id(&db.conn, source.folder_id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Source conversation folder was not found"))?;

    // Forking from a specific visible message is a bounded snapshot operation:
    // ACP's native method can only fork its current tail, never an arbitrary
    // historical point. Latest-tail requests prefer the native protocol.
    let native_candidate = request.fork_message_id.is_none()
        && source
            .external_id
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty());
    let fallback_reason;
    if native_candidate {
        let session_id = source.external_id.clone().unwrap_or_default();
        let native_attempt: Result<(String, String), AcpError> = async {
            let runtime_env = build_session_runtime_env(
                db,
                source.agent_type,
                Some(session_id.as_str()),
                data_dir,
            )
            .await?;
            let connection_id = manager
                .spawn_isolated_session(
                    source.agent_type,
                    Some(folder.path.clone()),
                    session_id,
                    runtime_env,
                    owner_label.clone(),
                    emitter.clone(),
                    request.preferred_mode_id.clone(),
                    request.preferred_config_values.clone(),
                )
                .await?;
            match manager.fork_protocol_only(&connection_id).await {
                Ok(result) => Ok((connection_id, result.forked_session_id)),
                Err(error) => {
                    let _ = manager.disconnect(&connection_id).await;
                    Err(error)
                }
            }
        }
        .await;
        match native_attempt {
            Ok((connection_id, forked_session_id)) => {
                let (branch, _) = conversation_branch_service::create_branch_row(
                    &db.conn,
                    &source,
                    Some(forked_session_id),
                    None,
                    "native",
                    None,
                )
                .await
                .map_err(AppCommandError::from)?;
                if let Err(error) = manager
                    .bind_connection_to_conversation(&connection_id, branch.id, branch.folder_id)
                    .await
                {
                    // The relation remains durable and can be restored later;
                    // do not delete a successfully-forked native session merely
                    // because the process-local binding event failed.
                    tracing::warn!(
                        branch_conversation_id = branch.id,
                        connection_id,
                        error = %error,
                        "[ACP][branch] native branch persisted but live bind failed"
                    );
                }
                emit_conversation_upsert(emitter, &db.conn, branch.id).await;
                return Ok(CreateConversationBranchResult {
                    branch_conversation_id: branch.id,
                    source_conversation_id: source.id,
                    folder_id: branch.folder_id,
                    connection_id: Some(connection_id),
                    fork_mode: "native".into(),
                    fallback_reason: None,
                });
            }
            Err(error) => fallback_reason = Some(error.to_string()),
        }
    } else if request.fork_message_id.is_some() {
        fallback_reason = Some("ACP native fork cannot target an earlier message".into());
    } else {
        fallback_reason = Some("source conversation has no resumable ACP session".into());
    }

    let snapshot = request
        .snapshot_context
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AppCommandError::invalid_input(
                "This agent cannot fork natively and no conversation snapshot was supplied",
            )
        })?;
    let (branch, _) = conversation_branch_service::create_branch_row(
        &db.conn,
        &source,
        None,
        request.fork_message_id,
        "snapshot",
        Some(snapshot),
    )
    .await
    .map_err(AppCommandError::from)?;
    // Snapshot branches also get their own process immediately. This applies
    // the source tab's mode/config at creation time and guarantees parallel
    // task state even before the first prompt; if process startup fails, the
    // durable branch remains valid and its tab can retry through normal ACP
    // connection lifecycle.
    let snapshot_connection_id =
        match build_session_runtime_env(db, source.agent_type, None, data_dir).await {
            Ok(runtime_env) => match manager
                .spawn_agent(
                    source.agent_type,
                    Some(folder.path.clone()),
                    None,
                    runtime_env,
                    owner_label,
                    emitter.clone(),
                    request.preferred_mode_id,
                    request.preferred_config_values,
                )
                .await
            {
                Ok(connection_id) => {
                    if let Err(error) = manager
                        .bind_connection_to_conversation(
                            &connection_id,
                            branch.id,
                            branch.folder_id,
                        )
                        .await
                    {
                        tracing::warn!(
                            branch_conversation_id = branch.id,
                            connection_id,
                            error = %error,
                            "[ACP][branch] snapshot branch live bind failed"
                        );
                    }
                    Some(connection_id)
                }
                Err(error) => {
                    tracing::warn!(
                        branch_conversation_id = branch.id,
                        error = %error,
                        "[ACP][branch] snapshot branch persisted; live startup deferred"
                    );
                    None
                }
            },
            Err(error) => {
                tracing::warn!(
                    branch_conversation_id = branch.id,
                    error = %error,
                    "[ACP][branch] snapshot branch persisted; runtime env unavailable"
                );
                None
            }
        };
    emit_conversation_upsert(emitter, &db.conn, branch.id).await;
    Ok(CreateConversationBranchResult {
        branch_conversation_id: branch.id,
        source_conversation_id: source.id,
        folder_id: branch.folder_id,
        connection_id: snapshot_connection_id,
        fork_mode: "snapshot".into(),
        fallback_reason,
    })
}

pub async fn merge_conversation_branch_core(
    db: &AppDatabase,
    emitter: &EventEmitter,
    branch_conversation_id: i32,
    request_id: String,
    summary: String,
    deliverable_ids: Vec<String>,
) -> Result<conversation_branch_service::MergeBranchResult, AppCommandError> {
    let result = conversation_branch_service::merge_branch(
        &db.conn,
        branch_conversation_id,
        request_id,
        summary,
        deliverable_ids,
    )
    .await
    .map_err(AppCommandError::from)?;
    emit_conversation_upsert(emitter, &db.conn, result.target_conversation_id).await;
    Ok(result)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_conversation_branch(
    db: tauri::State<'_, AppDatabase>,
    manager: tauri::State<'_, ConnectionManager>,
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    request: CreateConversationBranchRequest,
) -> Result<CreateConversationBranchResult, AppCommandError> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map(|path| crate::paths::resolve_effective_data_dir(&path))
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    create_conversation_branch_core(
        &db,
        &manager,
        &EventEmitter::Tauri(app),
        data_dir.as_path(),
        window.label().to_string(),
        request,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_conversation_branch_info(
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
) -> Result<Option<conversation_branch_service::ConversationBranchInfo>, AppCommandError> {
    conversation_branch_service::get_info(&db.conn, conversation_id)
        .await
        .map_err(AppCommandError::from)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn merge_conversation_branch(
    db: tauri::State<'_, AppDatabase>,
    app: tauri::AppHandle,
    branch_conversation_id: i32,
    request_id: String,
    summary: String,
    deliverable_ids: Vec<String>,
) -> Result<conversation_branch_service::MergeBranchResult, AppCommandError> {
    merge_conversation_branch_core(
        &db,
        &EventEmitter::Tauri(app),
        branch_conversation_id,
        request_id,
        summary,
        deliverable_ids,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::service::conversation_branch_service;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::models::AgentType;

    #[tokio::test]
    async fn exact_message_branch_uses_explicit_snapshot_fallback() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-message-branch").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let result = create_conversation_branch_core(
            &db,
            &ConnectionManager::new(),
            &EventEmitter::Noop,
            Path::new("/tmp/codeg-message-branch-data"),
            "test".into(),
            CreateConversationBranchRequest {
                source_conversation_id: source_id,
                fork_message_id: Some("message-4".into()),
                snapshot_context: Some("User: context through message four".into()),
                preferred_mode_id: Some("code".into()),
                preferred_config_values: BTreeMap::new(),
            },
        )
        .await
        .unwrap();

        assert_eq!(result.fork_mode, "snapshot");
        assert!(result
            .fallback_reason
            .as_deref()
            .unwrap_or_default()
            .contains("earlier message"));
        let info = conversation_branch_service::get_info(&db.conn, result.branch_conversation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(info.fork_message_id.as_deref(), Some("message-4"));
        assert_eq!(info.source_conversation_id, source_id);
    }
}
