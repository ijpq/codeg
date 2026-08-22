use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::acp::error::AcpError;
use crate::acp::manager::ConnectionManager;
use crate::acp::types::ConnectionStatus;
use crate::app_error::AppCommandError;
use crate::commands::acp::build_session_runtime_env;
use crate::commands::conversation_branch_context::{
    build_branch_inheritance_snapshot, BranchInheritanceSnapshot,
};
use crate::commands::conversations::emit_conversation_upsert;
use crate::commands::conversations::get_folder_conversation_core;
use crate::db::service::{
    artifact_service, conversation_branch_service, conversation_service, folder_service,
};
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
    /// A durable ACP session id. Snapshot fallbacks intentionally leave this
    /// empty until their first real prompt creates a resumable rollout.
    pub branch_session_id: Option<String>,
    pub session_ready: bool,
    pub prompt_ready: bool,
    pub lifecycle_state: String,
    pub fork_mode: String,
    pub inheritance_mode: String,
    pub inherited_message_count: i32,
    pub inheritance_truncated: bool,
    pub fallback_reason: Option<String>,
}

fn inheritance_record(
    snapshot: &BranchInheritanceSnapshot,
    source_session_id: Option<String>,
    branch_session_id: Option<String>,
    snapshot_context: Option<String>,
) -> conversation_branch_service::BranchInheritanceRecord {
    conversation_branch_service::BranchInheritanceRecord {
        source_session_id,
        branch_session_id,
        inheritance_mode: snapshot.inheritance_mode.clone(),
        inherited_message_count: snapshot.inherited_message_count,
        inherited_context_chars: snapshot.context_chars,
        inherited_estimated_tokens: snapshot.estimated_tokens,
        inheritance_compressed: snapshot.compressed,
        inheritance_truncated: snapshot.truncated,
        inheritance_note: snapshot.note.clone(),
        forked_through_at: snapshot.forked_through_at,
        snapshot_version: snapshot.snapshot_version,
        snapshot_context,
        snapshot_images: snapshot.images.clone(),
    }
}

fn ensure_independent_fork_session(
    source_session_id: &str,
    branch_session_id: &str,
) -> Result<(), AcpError> {
    if source_session_id == branch_session_id {
        return Err(AcpError::protocol(
            "ACP session/fork returned the source session instead of an independent session",
        ));
    }
    Ok(())
}

fn branch_session_start_error(context: &str, error: &AcpError) -> AppCommandError {
    AppCommandError::task_execution_failed(format!("{context}: {error}"))
}

fn should_attempt_native_fork(
    fork_message_id: Option<&str>,
    source_session_id: Option<&str>,
    source_live_busy: bool,
) -> bool {
    fork_message_id.is_none()
        && !source_live_busy
        && source_session_id.is_some_and(|id| !id.trim().is_empty())
}

fn stable_source_turn_count(turns: &[crate::models::MessageTurn]) -> usize {
    turns
        .iter()
        .rposition(|turn| {
            matches!(turn.role, crate::models::TurnRole::Assistant)
                && turn.completed_at.is_some()
        })
        .map_or(0, |index| index + 1)
}

pub async fn create_conversation_branch_core(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    data_dir: &Path,
    owner_label: String,
    request: CreateConversationBranchRequest,
) -> Result<CreateConversationBranchResult, AppCommandError> {
    // A stale `conversation.status = in_progress` must not force a latest-tail
    // branch down the lossy snapshot path. Reconcile durable runs first, then
    // use the live connection itself as the authority for whether the source
    // is genuinely busy. This makes the menu action prefer the exact same ACP
    // `session/fork` inheritance as Fork & Send whenever the source is idle.
    manager
        .reconcile_conversation_runs(&db.conn, request.source_conversation_id)
        .await
        .map_err(|error| {
            AppCommandError::task_execution_failed(format!(
                "Source conversation state could not be reconciled before branching: {error}"
            ))
        })?;
    let source = conversation_service::get_by_id(&db.conn, request.source_conversation_id)
        .await
        .map_err(AppCommandError::from)?;
    let folder = folder_service::get_folder_by_id(&db.conn, source.folder_id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Source conversation folder was not found"))?;

    // Forking from a specific visible message is a bounded snapshot operation:
    // ACP's native method can only fork its current tail, never an arbitrary
    // historical point. Latest-tail requests prefer the native protocol before
    // parsing or compressing the transcript, matching Fork & Send's exact,
    // full-session inheritance and keeping large native forks fast.
    let (source_connection_busy, idle_source_connection_id) =
        if let Some(connection_id) = manager.find_connection_by_conversation_id(source.id).await {
            if let Some(state) = manager.get_state(&connection_id).await {
                let state = state.read().await;
                let busy = state.turn_in_flight || state.status == ConnectionStatus::Prompting;
                let reusable_for_fork = !busy
                    && state.status == ConnectionStatus::Connected
                    && state.external_id.as_deref() == source.external_id.as_deref()
                    && state.agent_type == source.agent_type;
                (busy, reusable_for_fork.then_some(connection_id))
            } else {
                (false, None)
            }
        } else {
            (false, None)
        };
    // The live connection is authoritative when present, but a reconnect or
    // stale browser binding can temporarily hide it. Consult the durable turn
    // state as well so an in-flight round is never copied merely because its
    // connection could not be found in this process at this instant.
    let source_has_active_run =
        !artifact_service::active_runs_for_conversation(&db.conn, source.id)
            .await?
            .is_empty();
    let source_live_busy = source_connection_busy || source_has_active_run;
    let native_candidate = should_attempt_native_fork(
        request.fork_message_id.as_deref(),
        source.external_id.as_deref(),
        source_live_busy,
    );
    let mut use_stable_boundary = source_live_busy;
    let fallback_reason;
    if native_candidate {
        let session_id = source.external_id.clone().unwrap_or_default();
        // An idle source tab already owns the only writer Codex permits for
        // this thread. Reuse the exact same atomic two-row operation as Fork &
        // Send: the current row becomes S2 (the branch), a sibling row keeps S1
        // (the source), and the durable branch relation is committed in the
        // same transaction. Starting an isolated writer here used to race the
        // source tab's restore and produced `already has an active writer`.
        if let Some(connection_id) = idle_source_connection_id.clone() {
            match manager
                .fork_session(db, &connection_id, Some(source.id), Some(source.folder_id))
                .await
            {
                Ok(result) => {
                    if let Err(error) = ensure_independent_fork_session(
                        &result.original_session_id,
                        &result.forked_session_id,
                    ) {
                        return Err(branch_session_start_error(
                            "Native branch did not return an independent session",
                            &error,
                        ));
                    }
                    let inherited_message_count =
                        i32::try_from(source.message_count).unwrap_or(i32::MAX);
                    tracing::info!(
                        source_conversation_id = result.sibling_conversation_id,
                        branch_conversation_id = source.id,
                        source_session_id = result.original_session_id,
                        branch_session_id = result.forked_session_id,
                        connection_id,
                        inheritance_mode = "native_fork",
                        mapping_strategy = "fork_send_atomic_two_row",
                        "[ACP][branch] menu branch reused the live native fork mapping"
                    );
                    return Ok(CreateConversationBranchResult {
                        branch_conversation_id: source.id,
                        source_conversation_id: result.sibling_conversation_id,
                        folder_id: source.folder_id,
                        connection_id: Some(connection_id),
                        branch_session_id: Some(result.forked_session_id),
                        session_ready: true,
                        prompt_ready: true,
                        lifecycle_state: "ready".into(),
                        fork_mode: "native".into(),
                        inheritance_mode: "native_fork".into(),
                        inherited_message_count,
                        inheritance_truncated: false,
                        fallback_reason: None,
                    });
                }
                Err(error) => {
                    use_stable_boundary |= matches!(&error, AcpError::TurnInProgress);
                    fallback_reason = Some(error.to_string());
                }
            }
        } else {
            let native_attempt: Result<(String, String), AcpError> = async {
                // Native branching loads the source session on an isolated ACP
                // handle. Serialize that writer with persisted-conversation
                // restore; otherwise a tab switch during branch creation can
                // issue resume/load for the same Codex thread and both fail
                // with `already has an active writer`.
                let _session_operation_guard = manager
                    .lock_session_operation(
                        source.agent_type,
                        Some(Path::new(&folder.path)),
                        &session_id,
                    )
                    .await?;
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
                        session_id.clone(),
                        runtime_env,
                        owner_label.clone(),
                        emitter.clone(),
                        request.preferred_mode_id.clone(),
                        request.preferred_config_values.clone(),
                    )
                    .await?;
                match manager.fork_protocol_only(&connection_id).await {
                    Ok(result) => {
                        if let Err(error) = ensure_independent_fork_session(
                            &session_id,
                            &result.forked_session_id,
                        ) {
                            let _ = manager.disconnect(&connection_id).await;
                            Err(error)
                        } else {
                            Ok((connection_id, result.forked_session_id))
                        }
                    }
                    Err(error) => {
                        let _ = manager.disconnect(&connection_id).await;
                        Err(error)
                    }
                }
            }
            .await;
            match native_attempt {
                Ok((connection_id, forked_session_id)) => {
                    let inherited_message_count =
                        i32::try_from(source.message_count).unwrap_or(i32::MAX);
                    let (branch, _) = match conversation_branch_service::create_branch_row(
                        &db.conn,
                        &source,
                        Some(forked_session_id.clone()),
                        None,
                        "native",
                        conversation_branch_service::BranchInheritanceRecord {
                            source_session_id: source.external_id.clone(),
                            branch_session_id: Some(forked_session_id.clone()),
                            inheritance_mode: "native_fork".into(),
                            inherited_message_count,
                            inherited_context_chars: 0,
                            inherited_estimated_tokens: 0,
                            inheritance_compressed: false,
                            inheritance_truncated: false,
                            inheritance_note: Some(
                                "ACP session/fork created a distinct session with the complete native context; no CodeG snapshot was injected."
                                    .into()
                            ),
                            forked_through_at: Some(chrono::Utc::now()),
                            snapshot_version: 2,
                            snapshot_context: None,
                            snapshot_images: Vec::new(),
                        },
                    )
                    .await
                    {
                        Ok(created) => created,
                        Err(error) => {
                            // The isolated writer now points at the fork.
                            // Closing it leaves the source row on S1; the
                            // unreferenced S2 rollout remains auditable.
                            let _ = manager.disconnect(&connection_id).await;
                            return Err(AppCommandError::from(error));
                        }
                    };
                    if let Err(error) = manager
                        .bind_connection_to_conversation(
                            &connection_id,
                            branch.id,
                            branch.folder_id,
                        )
                        .await
                    {
                        let cleanup = conversation_branch_service::remove_incomplete_branch(
                            &db.conn, branch.id,
                        )
                        .await;
                        let _ = manager.disconnect(&connection_id).await;
                        tracing::error!(
                            branch_conversation_id = branch.id,
                            connection_id,
                            error = %error,
                            cleanup_error = ?cleanup.as_ref().err(),
                            "[ACP][branch] native branch bind failed; incomplete row removed"
                        );
                        return Err(branch_session_start_error(
                            "Branch session was created but could not be attached",
                            &error,
                        ));
                    }
                    emit_conversation_upsert(emitter, &db.conn, branch.id).await;
                    tracing::info!(
                        source_conversation_id = source.id,
                        branch_conversation_id = branch.id,
                        source_session_id = source.external_id,
                        branch_session_id = forked_session_id,
                        inheritance_mode = "native_fork",
                        "[ACP][branch] independent native branch persisted"
                    );
                    return Ok(CreateConversationBranchResult {
                        branch_conversation_id: branch.id,
                        source_conversation_id: source.id,
                        folder_id: branch.folder_id,
                        connection_id: Some(connection_id),
                        branch_session_id: Some(forked_session_id),
                        session_ready: true,
                        prompt_ready: true,
                        lifecycle_state: "ready".into(),
                        fork_mode: "native".into(),
                        inheritance_mode: "native_fork".into(),
                        inherited_message_count,
                        inheritance_truncated: false,
                        fallback_reason: None,
                    });
                }
                Err(error) => {
                    use_stable_boundary |= matches!(&error, AcpError::TurnInProgress);
                    fallback_reason = Some(error.to_string());
                }
            }
        }
    } else if request.fork_message_id.is_some() {
        fallback_reason = Some("ACP native fork cannot target an earlier message".into());
    } else if source_live_busy {
        fallback_reason = Some(
            "source conversation is generating; used its latest stable persisted boundary".into(),
        );
    } else {
        fallback_reason = Some("source conversation has no resumable ACP session".into());
    }

    // Native inheritance was unavailable or the user selected an earlier
    // message. Only now parse the authoritative persisted transcript and build
    // the honest replay/snapshot fallback. This is never sourced from the
    // browser's bounded history page, so every persisted message up to the
    // selected boundary participates before the context-budget rules apply.
    let (mut source_detail, _) = get_folder_conversation_core(&db.conn, source.id).await?;
    if source_detail.summary.origin_cwd.is_none() {
        source_detail.summary.origin_cwd = Some(folder.path.clone());
    }
    if use_stable_boundary && request.fork_message_id.is_none() {
        // Scheme A for the explicit "Create branch" action while the source
        // keeps running: snapshot only through the last fully committed
        // assistant turn. The current user prompt, partial assistant stream and
        // tool events belong to the in-flight round and are intentionally not
        // represented as completed context in the branch.
        let stable_len = stable_source_turn_count(&source_detail.turns);
        source_detail.turns.truncate(stable_len);
        tracing::info!(
            source_conversation_id = source.id,
            stable_message_count = stable_len,
            stage = "branch_stable_boundary_selected",
            "[ACP][branch] excluded the source's in-flight round from snapshot inheritance"
        );
    }
    let inheritance = match build_branch_inheritance_snapshot(
        &source_detail,
        request.fork_message_id.as_deref(),
        None,
    ) {
        Ok(snapshot) => snapshot,
        Err(error)
            if request
                .snapshot_context
                .as_deref()
                .is_some_and(|v| !v.trim().is_empty()) =>
        {
            // Compatibility for one release of older clients. New clients do
            // not send this field. Mark it explicitly so it cannot be mistaken
            // for a server-built complete replay.
            let context = request.snapshot_context.clone().unwrap_or_default();
            BranchInheritanceSnapshot {
                context_chars: context.chars().count() as i64,
                estimated_tokens: crate::commands::conversation_branch_context::estimate_tokens(
                    &context,
                ) as i64,
                source_context_chars: context.chars().count() as i64,
                source_estimated_tokens:
                    crate::commands::conversation_branch_context::estimate_tokens(&context) as i64,
                context,
                inheritance_mode: "structured_snapshot".into(),
                inherited_message_count: 0,
                compressed: true,
                truncated: true,
                note: Some(format!(
                    "Legacy client snapshot used because persisted boundary resolution failed: {error}"
                )),
                fork_message_id: request.fork_message_id.clone(),
                forked_through_at: None,
                snapshot_version: 1,
                images: Vec::new(),
            }
        }
        Err(error) => return Err(error),
    };
    tracing::info!(
        source_conversation_id = source.id,
        fork_message_id = ?inheritance.fork_message_id,
        inheritance_mode = inheritance.inheritance_mode,
        inherited_message_count = inheritance.inherited_message_count,
        inherited_context_chars = inheritance.context_chars,
        inherited_estimated_tokens = inheritance.estimated_tokens,
        source_context_chars = inheritance.source_context_chars,
        source_estimated_tokens = inheritance.source_estimated_tokens,
        inherited_image_count = inheritance.images.len(),
        compressed = inheritance.compressed,
        truncated = inheritance.truncated,
        snapshot_version = inheritance.snapshot_version,
        fallback_reason = ?fallback_reason,
        "[ACP][branch] authoritative snapshot fallback prepared"
    );

    // session/new does not create a durable Codex rollout until a real prompt
    // is accepted. Persist the inheritance as a provisional branch instead of
    // pretending that an idle in-memory session id can be resumed forever.
    // Opening the branch may prewarm a connection, but the first real user
    // prompt atomically injects this snapshot and promotes the session id.
    tracing::info!(
        source_conversation_id = source.id,
        source_generating = source.status == "in_progress",
        lifecycle_state = "snapshot_ready",
        stage = "branch_snapshot_persist_started",
        fallback_reason = ?fallback_reason,
        "[ACP][branch] persisting provisional snapshot branch"
    );
    let snapshot = inheritance.context.clone();
    let (branch, relation) = conversation_branch_service::create_branch_row(
        &db.conn,
        &source,
        None,
        inheritance.fork_message_id.clone(),
        "snapshot",
        inheritance_record(
            &inheritance,
            source.external_id.clone(),
            None,
            Some(snapshot),
        ),
    )
    .await
    .map_err(AppCommandError::from)?;
    emit_conversation_upsert(emitter, &db.conn, branch.id).await;
    tracing::info!(
        source_conversation_id = source.id,
        branch_conversation_id = branch.id,
        connection_id = ?Option::<String>::None,
        external_session_id = ?Option::<String>::None,
        snapshot_digest = ?relation.snapshot_digest,
        snapshot_consumed_at = ?relation.snapshot_consumed_at,
        lifecycle_state = relation.lifecycle_state,
        inheritance_mode = inheritance.inheritance_mode,
        inherited_message_count = inheritance.inherited_message_count,
        inherited_estimated_tokens = inheritance.estimated_tokens,
        truncated = inheritance.truncated,
        fallback_reason = ?fallback_reason,
        stage = "branch_ready",
        "[ACP][branch] provisional snapshot branch persisted"
    );
    Ok(CreateConversationBranchResult {
        branch_conversation_id: branch.id,
        source_conversation_id: source.id,
        folder_id: branch.folder_id,
        connection_id: None,
        branch_session_id: None,
        session_ready: false,
        prompt_ready: false,
        lifecycle_state: "provisional".into(),
        fork_mode: "snapshot".into(),
        inheritance_mode: inheritance.inheritance_mode,
        inherited_message_count: inheritance.inherited_message_count,
        inheritance_truncated: inheritance.truncated,
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

pub async fn get_conversation_branch_info_core(
    db: &AppDatabase,
    conversation_id: i32,
) -> Result<Option<conversation_branch_service::ConversationBranchInfo>, AppCommandError> {
    if let Some(info) = conversation_branch_service::get_info(&db.conn, conversation_id)
        .await
        .map_err(AppCommandError::from)?
    {
        return Ok(Some(info));
    }
    let conversation = match conversation_service::get_by_id(&db.conn, conversation_id).await {
        Ok(conversation) => conversation,
        Err(crate::db::error::DbError::Migration(message))
            if message.starts_with("Conversation not found:") =>
        {
            return Ok(None)
        }
        Err(error) => return Err(AppCommandError::from(error)),
    };
    if conversation.agent_type != crate::models::agent::AgentType::Codex
        || conversation.parent_id.is_some()
        || conversation
            .title
            .as_deref()
            .is_none_or(|title| !title.trim_start().starts_with("[Fork]"))
    {
        return Ok(None);
    }
    let Some(session_id) = conversation.external_id else {
        return Ok(None);
    };
    let parent_session_id = match tokio::task::spawn_blocking(move || {
        crate::parsers::codex::native_fork_parent_session_id(&session_id)
    })
    .await
    {
        Ok(parent) => parent,
        Err(error) => {
            tracing::warn!(
                conversation_id,
                error = %error,
                "[ACP][branch] legacy fork relation lookup task failed"
            );
            None
        }
    };
    let Some(parent_session_id) = parent_session_id else {
        return Ok(None);
    };
    conversation_branch_service::repair_native_fork_relation(
        &db.conn,
        conversation_id,
        &parent_session_id,
    )
    .await
    .map_err(AppCommandError::from)
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
    get_conversation_branch_info_core(&db, conversation_id).await
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
    use crate::db::entities::{conversation, conversation_branch};
    use crate::db::service::conversation_branch_service;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::models::{AgentType, ContentBlock, MessageTurn, TurnRole};
    use sea_orm::{EntityTrait, PaginatorTrait};

    #[test]
    fn native_fork_must_return_a_distinct_session() {
        assert!(ensure_independent_fork_session("source", "branch").is_ok());
        assert!(ensure_independent_fork_session("same", "same").is_err());
    }

    #[test]
    fn latest_tail_uses_native_fork_when_the_live_session_is_idle() {
        assert!(should_attempt_native_fork(
            None,
            Some("source-session"),
            false
        ));
        assert!(!should_attempt_native_fork(
            Some("older-message"),
            Some("source-session"),
            false
        ));
        assert!(!should_attempt_native_fork(
            None,
            Some("source-session"),
            true
        ));
        assert!(!should_attempt_native_fork(None, None, false));
    }

    #[test]
    fn generating_source_boundary_excludes_the_entire_in_flight_round() {
        let turn = |id: &str, role: TurnRole, completed: bool| MessageTurn {
            id: id.into(),
            role,
            blocks: vec![ContentBlock::Text { text: id.into() }],
            timestamp: chrono::Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: completed.then(chrono::Utc::now),
        };
        let turns = vec![
            turn("stable-user", TurnRole::User, true),
            turn("stable-assistant", TurnRole::Assistant, true),
            turn("current-user", TurnRole::User, true),
            turn("partial-assistant", TurnRole::Assistant, false),
        ];
        assert_eq!(stable_source_turn_count(&turns), 2);

        let first_round = vec![
            turn("first-user", TurnRole::User, true),
            turn("partial-assistant", TurnRole::Assistant, false),
        ];
        assert_eq!(stable_source_turn_count(&first_round), 0);
    }

    #[tokio::test]
    async fn snapshot_branch_creation_does_not_require_a_live_agent() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-message-branch").await;
        let unavailable_agent = AgentType::custom("branch-ready-missing-agent").unwrap();
        let source_id = seed_conversation(&db, folder_id, unavailable_agent).await;
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
        .expect("snapshot persistence is independent of ACP availability");

        assert_eq!(result.lifecycle_state, "provisional");
        assert!(!result.session_ready);
        assert!(!result.prompt_ready);
        assert!(result.connection_id.is_none());
        assert!(result.branch_session_id.is_none());
        assert_eq!(
            conversation_branch::Entity::find()
                .count(&db.conn)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            conversation::Entity::find().count(&db.conn).await.unwrap(),
            2,
            "the source and durable provisional branch both remain"
        );
        let info = conversation_branch_service::get_info(&db.conn, result.branch_conversation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(info.lifecycle_state, "provisional");
        assert!(info.snapshot_consumed_at.is_none());
    }

    #[tokio::test]
    async fn durable_active_run_forces_a_stable_snapshot_without_loading_the_writer() {
        let db = fresh_in_memory_db().await;
        let folder_path = "/tmp/codeg-running-source-branch";
        let folder_id = seed_folder(&db, folder_path).await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        conversation_service::bind_external_id(&db.conn, source_id, "busy-source-session", &[])
            .await
            .unwrap();
        artifact_service::create_run(
            &db.conn,
            artifact_service::NewTurnRun {
                id: "running-source-turn".into(),
                conversation_id: source_id,
                connection_id: "writer-owned-elsewhere".into(),
                client_message_id: Some("running-source-message".into()),
                prompt_fingerprint: None,
                folder_id: Some(folder_id),
                root_path: folder_path.into(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
                expectation_json: "{}".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            artifact_service::active_runs_for_conversation(&db.conn, source_id)
                .await
                .unwrap()
                .len(),
            1,
            "the durable running turn must participate in branch busy detection"
        );

        let result = create_conversation_branch_core(
            &db,
            &ConnectionManager::new(),
            &EventEmitter::Noop,
            Path::new("/tmp/codeg-running-source-branch-data"),
            "test".into(),
            CreateConversationBranchRequest {
                source_conversation_id: source_id,
                fork_message_id: None,
                snapshot_context: Some("last committed context".into()),
                preferred_mode_id: None,
                preferred_config_values: BTreeMap::new(),
            },
        )
        .await
        .expect("a durable running turn must not try to attach a second writer");

        assert_eq!(result.lifecycle_state, "provisional");
        assert_eq!(result.inheritance_mode, "full_replay");
        assert!(
            result.fallback_reason.is_some(),
            "a busy source must record why native fork was skipped"
        );
        assert_eq!(
            conversation_service::get_by_id(&db.conn, source_id)
                .await
                .unwrap()
                .external_id
                .as_deref(),
            Some("busy-source-session")
        );
    }

    #[tokio::test]
    async fn menu_branch_reuses_idle_writer_and_the_fork_send_mapping() {
        use crate::acp::connection::ConnectionCommand;

        let db = fresh_in_memory_db().await;
        let folder_path = "/tmp/codeg-menu-native-branch";
        let folder_id = seed_folder(&db, folder_path).await;
        let source_id = seed_conversation(&db, folder_id, AgentType::ClaudeCode).await;
        conversation_service::bind_external_id(&db.conn, source_id, "session-source", &[])
            .await
            .unwrap();

        let manager = ConnectionManager::new();
        let mut commands = manager
            .insert_test_connection_live(
                "idle-source-writer",
                AgentType::ClaudeCode,
                Some(std::path::PathBuf::from(folder_path)),
                EventEmitter::Noop,
            )
            .await;
        let state = manager.get_state("idle-source-writer").await.unwrap();
        {
            let mut state = state.write().await;
            state.external_id = Some("session-source".into());
            state.conversation_id = Some(source_id);
            state.folder_id = Some(folder_id);
        }
        let fork_reply = tokio::spawn(async move {
            if let Some(ConnectionCommand::Fork { reply }) = commands.recv().await {
                let _ = reply.send(Ok(crate::acp::types::ForkProtocolResult {
                    forked_session_id: "session-branch".into(),
                    original_session_id: "session-source".into(),
                }));
            }
        });

        let result = create_conversation_branch_core(
            &db,
            &manager,
            &EventEmitter::Noop,
            Path::new("/tmp/codeg-menu-native-branch-data"),
            "test".into(),
            CreateConversationBranchRequest {
                source_conversation_id: source_id,
                fork_message_id: None,
                snapshot_context: None,
                preferred_mode_id: None,
                preferred_config_values: BTreeMap::new(),
            },
        )
        .await
        .expect("menu branch should fork the existing writer");
        fork_reply.await.unwrap();

        assert_eq!(result.branch_conversation_id, source_id);
        assert_ne!(result.source_conversation_id, source_id);
        assert_eq!(result.branch_session_id.as_deref(), Some("session-branch"));
        let branch = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();
        let source = conversation_service::get_by_id(&db.conn, result.source_conversation_id)
            .await
            .unwrap();
        assert_eq!(branch.external_id.as_deref(), Some("session-branch"));
        assert_eq!(source.external_id.as_deref(), Some("session-source"));
        let relation = conversation_branch_service::get_info(&db.conn, source_id)
            .await
            .unwrap()
            .expect("fork-send relation");
        assert_eq!(relation.source_conversation_id, result.source_conversation_id);
        assert_eq!(relation.inheritance_mode, "native_fork");
    }
}
