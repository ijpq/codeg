use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, LazyLock};

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

static BRANCH_CREATION_FLIGHTS: LazyLock<
    tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
> = LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));

async fn lock_branch_creation_request(request_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut flights = BRANCH_CREATION_FLIGHTS.lock().await;
        // The map retains one Arc after a completed request. Drop those idle
        // keys opportunistically so long-running servers do not accumulate an
        // entry for every branch ever created; active guards/waiters keep the
        // strong count above one and are never removed.
        flights.retain(|_, flight| Arc::strong_count(flight) > 1);
        flights
            .entry(request_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    lock.lock_owned().await
}
#[cfg(feature = "tauri-runtime")]
use tauri::Manager;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversationBranchRequest {
    /// Stable idempotency key shared by Create Branch and Fork & Send. A lost
    /// HTTP response or a reloaded persisted queue must resolve to the same
    /// branch instead of forking twice.
    pub request_id: Option<String>,
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
            matches!(turn.role, crate::models::TurnRole::Assistant) && turn.completed_at.is_some()
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
    let creation_request_id = request
        .request_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| Some(uuid::Uuid::new_v4().to_string()));
    // The idempotency row does not exist until a potentially long native fork
    // has created and verified S2. Single-flight the whole protocol operation,
    // then re-read the durable result below; otherwise two retries with one
    // client id could create two child sessions before the unique index becomes
    // able to arbitrate.
    let _creation_guard = match creation_request_id.as_deref() {
        Some(request_id) => Some(lock_branch_creation_request(request_id).await),
        None => None,
    };
    if let Some(request_id) = creation_request_id.as_deref() {
        if let Some(existing) =
            conversation_branch_service::get_by_creation_request_id(&db.conn, request_id)
                .await
                .map_err(AppCommandError::from)?
        {
            let branch = conversation_service::get_by_id(&db.conn, existing.branch_conversation_id)
                .await
                .map_err(AppCommandError::from)?;
            let connection_id = manager
                .find_connection_by_conversation_id(existing.branch_conversation_id)
                .await;
            let prompt_ready = if let Some(connection_id) = connection_id.as_deref() {
                manager.get_state(connection_id).await.is_some_and(|state| {
                    state
                        .try_read()
                        .is_ok_and(|state| state.status == ConnectionStatus::Connected)
                })
            } else {
                false
            };
            tracing::info!(
                creation_request_id = request_id,
                source_conversation_id = existing.source_conversation_id,
                branch_conversation_id = existing.branch_conversation_id,
                lifecycle_state = existing.lifecycle_state,
                "[ACP][branch] deduplicated branch creation request"
            );
            return Ok(CreateConversationBranchResult {
                branch_conversation_id: existing.branch_conversation_id,
                source_conversation_id: existing.source_conversation_id,
                folder_id: branch.folder_id,
                connection_id,
                branch_session_id: existing.branch_session_id,
                session_ready: existing.session_verified_at.is_some(),
                prompt_ready,
                lifecycle_state: existing.lifecycle_state,
                fork_mode: existing.fork_mode,
                inheritance_mode: existing.inheritance_mode,
                inherited_message_count: existing.inherited_message_count,
                inheritance_truncated: existing.inheritance_truncated,
                fallback_reason: existing.lifecycle_error,
            });
        }
    }
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
        let native_attempt: Result<(String, String), AcpError> = async {
            // One durable Codex thread may have only one writer.  Serialize the
            // whole handoff by external session id, fork on the current writer
            // when it exists, and retire that adapter before loading S2 in a
            // fresh process.  The source conversation row is never renamed or
            // rebound and can restore S1 normally after this lock is released.
            let _source_session_guard = manager
                .lock_session_operation(
                    source.agent_type,
                    Some(Path::new(&folder.path)),
                    &session_id,
                )
                .await?;
            let source_connection_id =
                if let Some(connection_id) = idle_source_connection_id.clone() {
                    connection_id
                } else {
                    let runtime_env = build_session_runtime_env(
                        db,
                        source.agent_type,
                        Some(session_id.as_str()),
                        data_dir,
                    )
                    .await?;
                    manager
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
                        .await?
                };
            let protocol_result = match manager.fork_protocol_detached(&source_connection_id).await
            {
                Ok(result) => result,
                Err(error) => {
                    if idle_source_connection_id.is_none() {
                        let _ = manager.disconnect(&source_connection_id).await;
                    }
                    return Err(error);
                }
            };
            ensure_independent_fork_session(
                &protocol_result.original_session_id,
                &protocol_result.forked_session_id,
            )?;

            let branch_session_id = protocol_result.forked_session_id;
            let _branch_session_guard = manager
                .lock_session_operation(
                    source.agent_type,
                    Some(Path::new(&folder.path)),
                    &branch_session_id,
                )
                .await?;
            let runtime_env = build_session_runtime_env(
                db,
                source.agent_type,
                Some(branch_session_id.as_str()),
                data_dir,
            )
            .await?;
            let branch_connection_id = manager
                .spawn_isolated_session(
                    source.agent_type,
                    Some(folder.path.clone()),
                    branch_session_id.clone(),
                    runtime_env,
                    owner_label.clone(),
                    emitter.clone(),
                    request.preferred_mode_id.clone(),
                    request.preferred_config_values.clone(),
                )
                .await?;
            Ok((branch_connection_id, branch_session_id))
        }
        .await;
        match native_attempt {
            Ok((connection_id, forked_session_id)) => {
                let inherited_message_count =
                    i32::try_from(source.message_count).unwrap_or(i32::MAX);
                let persisted = conversation_branch_service::create_branch_row_with_request(
                    &db.conn,
                    &source,
                    creation_request_id.clone(),
                    Some(forked_session_id.clone()),
                    None,
                    "native",
                    inheritance_record(
                        &BranchInheritanceSnapshot {
                            context: String::new(),
                            inheritance_mode: "native_fork".into(),
                            inherited_message_count,
                            context_chars: 0,
                            estimated_tokens: 0,
                            source_context_chars: 0,
                            source_estimated_tokens: 0,
                            compressed: false,
                            truncated: false,
                            note: Some(
                                "ACP session/fork created an independent session with the complete native context."
                                    .into(),
                            ),
                            fork_message_id: None,
                            forked_through_at: Some(chrono::Utc::now()),
                            snapshot_version: 2,
                            images: Vec::new(),
                        },
                        source.external_id.clone(),
                        Some(forked_session_id.clone()),
                        None,
                    ),
                )
                .await;
                let (branch, _) = match persisted {
                    Ok(persisted) => persisted,
                    Err(error) => {
                        let connection_cleanup = manager.disconnect(&connection_id).await;
                        tracing::error!(
                            source_conversation_id = source.id,
                            branch_session_id = forked_session_id,
                            connection_id,
                            error = %error,
                            connection_cleanup_error = ?connection_cleanup.as_ref().err(),
                            "[ACP][branch] native session was verified but its atomic database mapping failed"
                        );
                        return Err(AppCommandError::from(error));
                    }
                };
                if let Err(error) = manager
                    .bind_connection_to_conversation(&connection_id, branch.id, branch.folder_id)
                    .await
                {
                    let cleanup =
                        conversation_branch_service::remove_incomplete_branch(&db.conn, branch.id)
                            .await;
                    let _ = manager.disconnect(&connection_id).await;
                    tracing::error!(
                        branch_conversation_id = branch.id,
                        connection_id,
                        error = %error,
                        cleanup_error = ?cleanup.as_ref().err(),
                        "[ACP][branch] verified native branch bind failed; row rolled back"
                    );
                    return Err(branch_session_start_error(
                        "Branch session was verified but could not be attached",
                        &error,
                    ));
                }
                emit_conversation_upsert(emitter, &db.conn, source.id).await;
                emit_conversation_upsert(emitter, &db.conn, branch.id).await;
                tracing::info!(
                    source_conversation_id = source.id,
                    branch_conversation_id = branch.id,
                    source_session_id = source.external_id,
                    branch_session_id = forked_session_id,
                    connection_id,
                    mapping_strategy = "immutable_source_detached_writer_handoff",
                    "[ACP][branch] verified native branch persisted"
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
    let (branch, relation) = conversation_branch_service::create_branch_row_with_request(
        &db.conn,
        &source,
        creation_request_id,
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
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    branch_conversation_id: i32,
    request_id: String,
) -> Result<conversation_branch_service::MergeBranchResult, AppCommandError> {
    tracing::info!(
        branch_conversation_id,
        merge_request_id = request_id,
        stage = "branch_merge_started",
        "[ACP][branch] one-click return to source started"
    );
    manager
        .reconcile_conversation_runs(&db.conn, branch_conversation_id)
        .await
        .map_err(|error| {
            AppCommandError::task_execution_failed(format!(
                "The branch state could not be reconciled before returning to its source: {error}"
            ))
        })?;
    let branch_connection_id = manager
        .find_connection_by_conversation_id(branch_conversation_id)
        .await;
    if let Some(connection_id) = branch_connection_id.as_deref() {
        let is_running = manager.get_state(connection_id).await.is_some_and(|state| {
            state.try_read().is_ok_and(|state| {
                state.turn_in_flight || state.status == ConnectionStatus::Prompting
            })
        });
        if is_running {
            manager
                .cancel(&db.conn, connection_id)
                .await
                .map_err(|error| {
                    AppCommandError::task_execution_failed(format!(
                        "The branch could not be stopped before returning to its source: {error}"
                    ))
                })?;
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(35);
            loop {
                if artifact_service::active_runs_for_conversation(&db.conn, branch_conversation_id)
                    .await?
                    .is_empty()
                {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(AppCommandError::task_execution_failed(
                        "The branch is still stopping; its results were preserved. Retry return to source shortly.",
                    ));
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
    if !artifact_service::active_runs_for_conversation(&db.conn, branch_conversation_id)
        .await?
        .is_empty()
    {
        return Err(AppCommandError::task_execution_failed(
            "The branch still has an active turn without a controllable connection; its results were preserved. Retry after restoring the branch.",
        ));
    }

    let (detail, _) = get_folder_conversation_core(&db.conn, branch_conversation_id).await?;
    let inherited = detail
        .branch_history
        .as_ref()
        .map_or(0, |history| history.inherited_turn_count)
        .min(detail.turns.len());
    let branch_title = detail.summary.title.as_deref().unwrap_or("未命名分支");
    let mut sections = Vec::new();
    for turn in detail.turns.iter().skip(inherited) {
        let text = turn
            .blocks
            .iter()
            .filter_map(|block| match block {
                crate::models::ContentBlock::Text { text } if !text.trim().is_empty() => {
                    Some(text.trim())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            continue;
        }
        let label = match turn.role {
            crate::models::TurnRole::User => "用户",
            crate::models::TurnRole::Assistant => "助手",
            _ => continue,
        };
        sections.push(format!("### {label}\n{text}"));
    }
    let mut seen_changes = std::collections::HashSet::new();
    let mut file_changes = Vec::new();
    for run in detail
        .artifact_runs
        .iter()
        .filter(|run| run.conversation_id == branch_conversation_id)
    {
        for change in &run.changes {
            let path = change.path.trim();
            if path.is_empty() {
                continue;
            }
            let old_path = change.old_path.as_deref().map(str::trim).unwrap_or("");
            let identity = (change.kind.as_str(), old_path, path);
            if !seen_changes.insert(identity) {
                continue;
            }
            let path = path.replace(['\r', '\n'], " ");
            let old_path = old_path.replace(['\r', '\n'], " ");
            let description = match (change.kind.as_str(), old_path.is_empty()) {
                ("created", _) => format!("创建 `{path}`"),
                ("modified", _) => format!("修改 `{path}`"),
                ("deleted", _) => format!("删除 `{path}`"),
                ("renamed", false) => format!("移动 `{old_path}` → `{path}`"),
                (kind, _) => format!("{kind} `{path}`"),
            };
            file_changes.push(format!("- {description}"));
        }
    }
    if !file_changes.is_empty() {
        sections.push(format!("### 文件变更\n{}", file_changes.join("\n")));
    }
    let summary = format!(
        "## 分支回归：{branch_title}\n\n合并时间：{}\n\n{}",
        chrono::Utc::now().to_rfc3339(),
        if sections.is_empty() {
            "分支没有新增可见消息；有效产物和文件引用仍已一并回传。".to_string()
        } else {
            sections.join("\n\n")
        }
    );
    let result = conversation_branch_service::merge_branch(
        &db.conn,
        branch_conversation_id,
        request_id,
        summary,
        Vec::new(),
    )
    .await
    .map_err(AppCommandError::from)?;
    if let Some(connection_id) = branch_connection_id.as_deref() {
        if let Err(error) = manager.disconnect(connection_id).await {
            tracing::warn!(
                branch_conversation_id,
                connection_id,
                error = %error,
                stage = "merged_branch_connection_cleanup_failed",
                "[ACP][branch] merge committed; idle branch connection cleanup will retry via sweep"
            );
        }
    }
    emit_conversation_upsert(emitter, &db.conn, result.target_conversation_id).await;
    // A merged branch remains queryable/auditable in the database but leaves
    // every client's active workspace immediately. Reuse the tab-version
    // barrier so an older debounced save cannot resurrect its tab after a
    // reload, and use the list's removal event without soft-deleting the row.
    crate::commands::conversations::cleanup_tabs_for_deleted_conversation(
        emitter,
        &db.conn,
        branch_conversation_id,
    )
    .await;
    crate::commands::conversations::emit_conversation_deleted(emitter, branch_conversation_id);
    tracing::info!(
        branch_conversation_id,
        source_conversation_id = result.target_conversation_id,
        merge_id = result.merge_id,
        copied_deliverable_count = result.copied_deliverable_count,
        deduplicated = result.deduplicated,
        lifecycle_state = "merged",
        stage = "branch_merge_completed",
        "[ACP][branch] one-click return to source committed"
    );
    Ok(result)
}

pub async fn get_conversation_branch_info_core(
    db: &AppDatabase,
    conversation_id: i32,
) -> Result<Option<conversation_branch_service::ConversationBranchInfo>, AppCommandError> {
    conversation_branch_service::repair_empty_snapshot_as_provisional(&db.conn, conversation_id)
        .await
        .map_err(AppCommandError::from)?;
    conversation_branch_service::normalize_legacy_branch_metadata(&db.conn, conversation_id)
        .await
        .map_err(AppCommandError::from)?;
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
    let repaired = conversation_branch_service::repair_native_fork_relation(
        &db.conn,
        conversation_id,
        &parent_session_id,
    )
    .await
    .map_err(AppCommandError::from)?;
    if repaired.is_some() {
        conversation_branch_service::normalize_legacy_branch_metadata(&db.conn, conversation_id)
            .await
            .map_err(AppCommandError::from)?;
        return conversation_branch_service::get_info(&db.conn, conversation_id)
            .await
            .map_err(AppCommandError::from);
    }
    Ok(None)
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
    let owned_db = AppDatabase {
        conn: db.conn.clone(),
    };
    let owned_manager = manager.clone_ref();
    let emitter = EventEmitter::Tauri(app);
    let owner_label = window.label().to_string();
    tokio::spawn(async move {
        create_conversation_branch_core(
            &owned_db,
            &owned_manager,
            &emitter,
            data_dir.as_path(),
            owner_label,
            request,
        )
        .await
    })
    .await
    .map_err(|error| {
        AppCommandError::task_execution_failed(format!(
            "Branch creation task stopped unexpectedly: {error}"
        ))
    })?
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
    manager: tauri::State<'_, ConnectionManager>,
    app: tauri::AppHandle,
    branch_conversation_id: i32,
    request_id: String,
) -> Result<conversation_branch_service::MergeBranchResult, AppCommandError> {
    merge_conversation_branch_core(
        &db,
        &manager,
        &EventEmitter::Tauri(app),
        branch_conversation_id,
        request_id,
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
                request_id: None,
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
                request_id: None,
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
    async fn native_fork_handoff_retires_the_source_writer() {
        use crate::acp::connection::ConnectionCommand;

        let folder_path = "/tmp/codeg-menu-native-branch";
        let manager = ConnectionManager::new();
        let mut commands = manager
            .insert_test_connection_live(
                "idle-source-writer",
                AgentType::Codex,
                Some(std::path::PathBuf::from(folder_path)),
                EventEmitter::Noop,
            )
            .await;
        let state = manager.get_state("idle-source-writer").await.unwrap();
        {
            let mut state = state.write().await;
            state.external_id = Some("session-source".into());
        }
        let manager_for_exit = manager.clone_ref();
        let fork_reply = tokio::spawn(async move {
            if let Some(ConnectionCommand::ForkDetached { reply }) = commands.recv().await {
                let _ = reply.send(Ok(crate::acp::types::ForkProtocolResult {
                    forked_session_id: "session-branch".into(),
                    original_session_id: "session-source".into(),
                }));
                let _ = manager_for_exit.disconnect("idle-source-writer").await;
            }
        });

        let result = manager
            .fork_protocol_detached("idle-source-writer")
            .await
            .expect("detached fork result");
        fork_reply.await.unwrap();

        assert_eq!(result.original_session_id, "session-source");
        assert_eq!(result.forked_session_id, "session-branch");
        assert!(manager.get_state("idle-source-writer").await.is_none());
    }
}
