use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize};

use crate::acp::error::AcpError;
use crate::acp::manager::ConnectionManager;
use crate::acp::types::ConnectionStatus;
use crate::app_error::{AppCommandError, AppErrorCode};
use crate::commands::acp::build_session_runtime_env;
use crate::commands::conversation_branch_context::{
    build_branch_inheritance_snapshot, BranchInheritanceSnapshot,
};
use crate::commands::conversations::emit_conversation_upsert;
use crate::commands::conversations::{
    get_folder_conversation_core, get_folder_conversation_raw_core, strip_branch_merge_context,
    strip_branch_snapshot_context,
};
use crate::db::service::{
    artifact_service, conversation_branch_service, conversation_service, folder_service,
};
use crate::db::AppDatabase;
use crate::web::event_bridge::emit_event;
use crate::web::event_bridge::EventEmitter;

static BRANCH_CREATION_FLIGHTS: LazyLock<
    tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
> = LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));
static BRANCH_MERGE_FLIGHTS: LazyLock<
    tokio::sync::Mutex<std::collections::HashMap<i32, Arc<tokio::sync::Mutex<()>>>>,
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

async fn lock_branch_merge(conversation_id: i32) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut flights = BRANCH_MERGE_FLIGHTS.lock().await;
        flights.retain(|_, flight| Arc::strong_count(flight) > 1);
        flights
            .entry(conversation_id)
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
    /// Stable identity of the user's branch action. Unlike an individual HTTP
    /// request id, this survives component remounts and transport retries.
    /// The server validates its source/fork point before deduplicating it.
    pub operation_id: Option<String>,
    pub source_conversation_id: i32,
    pub fork_message_id: Option<String>,
    pub snapshot_context: Option<String>,
    /// Queue-backed UI entry points set this so the server, rather than stale
    /// browser runtime state, is authoritative about whether the source turn
    /// has reached a durable terminal state. A busy source returns the normal
    /// recoverable `turn_in_progress` conflict without creating a partial
    /// branch; the stable request id can be retried after the turn settles.
    #[serde(default)]
    pub defer_if_source_busy: bool,
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
    source_rollout_offset: Option<i64>,
    branch_rollout_offset: Option<i64>,
    fork_boundary_kind: Option<String>,
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
        source_rollout_offset,
        branch_rollout_offset,
        fork_boundary_kind,
        snapshot_version: snapshot.snapshot_version,
        snapshot_context,
        snapshot_images: snapshot.images.clone(),
    }
}

#[derive(Debug)]
struct NativeForkBoundary {
    fork_message_id: Option<String>,
    forked_through_at: chrono::DateTime<chrono::Utc>,
    source_rollout_offset: Option<i64>,
    branch_rollout_offset: Option<i64>,
    kind: String,
}

fn checked_rollout_offset(offset: u64) -> Result<i64, AcpError> {
    i64::try_from(offset)
        .map_err(|_| AcpError::protocol("Codex rollout offset exceeds SQLite INTEGER range"))
}

async fn capture_codex_source_boundary(
    session_id: String,
    cwd: String,
) -> Result<(Option<String>, chrono::DateTime<chrono::Utc>, i64), AcpError> {
    tokio::task::spawn_blocking(move || {
        let parser = crate::parsers::codex::CodexParser::new();
        let offset = checked_rollout_offset(
            parser
                .rollout_len(&session_id)
                .map_err(|error| AcpError::protocol(error.to_string()))?,
        )?;
        // The byte offset is the authoritative boundary. Resolving a friendly
        // visible turn id is deliberately bounded as well: a final tool record
        // can be huge, and branch creation must never parse an unbounded tail
        // merely to decorate the offset.
        let page = parser
            .get_conversation_page_bounded(&session_id, None, 1, Some(cwd), 16 * 1024 * 1024)
            .ok();
        let boundary = page.as_ref().and_then(|page| page.detail.turns.last());
        Ok((
            Some(
                boundary
                    .map(|turn| turn.id.clone())
                    .unwrap_or_else(|| format!("codex-rollout-offset-{offset}")),
            ),
            boundary
                .map(|turn| turn.timestamp)
                .unwrap_or_else(chrono::Utc::now),
            offset,
        ))
    })
    .await
    .map_err(|error| AcpError::protocol(format!("Codex boundary task failed: {error}")))?
}

async fn capture_codex_rollout_offset(session_id: String) -> Result<i64, AcpError> {
    tokio::task::spawn_blocking(move || {
        checked_rollout_offset(
            crate::parsers::codex::CodexParser::new()
                .rollout_len(&session_id)
                .map_err(|error| AcpError::protocol(error.to_string()))?,
        )
    })
    .await
    .map_err(|error| AcpError::protocol(format!("Codex boundary task failed: {error}")))?
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

fn source_busy_branch_error() -> AppCommandError {
    AppCommandError::new(
        AppErrorCode::TurnInProgress,
        "turn already in progress for source conversation; branch request remains queued",
    )
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
    let operation_id = request
        .operation_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| creation_request_id.clone());
    // The idempotency row does not exist until a potentially long native fork
    // has created and verified S2. Single-flight the whole protocol operation,
    // then re-read the durable result below; otherwise two retries with one
    // client id could create two child sessions before the unique index becomes
    // able to arbitrate.
    let _creation_guard = match operation_id.as_deref().or(creation_request_id.as_deref()) {
        Some(request_id) => Some(lock_branch_creation_request(request_id).await),
        None => None,
    };
    let existing_by_operation = match operation_id.as_deref() {
        Some(operation_id) => {
            conversation_branch_service::get_by_operation_id(&db.conn, operation_id)
                .await
                .map_err(AppCommandError::from)?
        }
        None => None,
    };
    let existing = if existing_by_operation.is_some() {
        existing_by_operation
    } else if let Some(request_id) = creation_request_id.as_deref() {
        conversation_branch_service::get_by_creation_request_id(&db.conn, request_id)
            .await
            .map_err(AppCommandError::from)?
    } else {
        None
    };
    if let Some(existing) = existing {
        if existing.source_conversation_id != request.source_conversation_id
            || existing.fork_message_id != request.fork_message_id
        {
            return Err(AppCommandError::invalid_input(
                "Branch operation id was already used for another source or fork point",
            ));
        }
        if conversation_branch_service::branch_conversation_is_deleted(
            &db.conn,
            existing.branch_conversation_id,
        )
        .await
        .map_err(AppCommandError::from)?
        {
            return Err(AppCommandError::not_found(
                "Branch operation was cancelled or its provisional conversation was deleted",
            ));
        }
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
            creation_request_id = ?creation_request_id,
            operation_id = ?operation_id,
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
    let source_active_runs =
        artifact_service::active_runs_for_conversation(&db.conn, source.id).await?;
    let source_has_active_run = !source_active_runs.is_empty();
    // Prompt admission creates the durable run before exposing the live ACP
    // generation. Therefore the run row is the authoritative queue blocker.
    // A lone stale Prompting/turn_in_flight bit (common after a missed terminal
    // event) must not strand Create Branch forever; it still prevents reusing
    // that particular connection below, so native fork will either take a safe
    // isolated writer handoff or fall back honestly to the persisted snapshot.
    let source_live_busy = source_has_active_run;
    if request.defer_if_source_busy && source_live_busy {
        tracing::info!(
            creation_request_id = ?creation_request_id,
            operation_id = ?operation_id,
            source_conversation_id = source.id,
            source_turn_run_id = ?source_active_runs.first().map(|run| run.id.as_str()),
            source_turn_state = ?source_active_runs.first().map(|run| &run.status),
            source_connection_busy,
            source_has_active_run,
            queue_state_before = "creating",
            queue_state_after = "queued",
            blocking_reason = "source_turn_active",
            trigger = "authoritative_branch_admission",
            "[ACP][branch-queue] source is active; durable-idempotent request deferred"
        );
        return Err(source_busy_branch_error());
    }
    let native_candidate = should_attempt_native_fork(
        request.fork_message_id.as_deref(),
        source.external_id.as_deref(),
        source_live_busy,
    );
    let mut use_stable_boundary = source_live_busy;
    let fallback_reason;
    if native_candidate {
        let session_id = source.external_id.clone().unwrap_or_default();
        let native_attempt: Result<(String, String, NativeForkBoundary), AcpError> = async {
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
            let source_boundary = if source.agent_type == crate::models::AgentType::Codex {
                let (fork_message_id, forked_through_at, source_rollout_offset) =
                    capture_codex_source_boundary(session_id.clone(), folder.path.clone()).await?;
                Some((fork_message_id, forked_through_at, source_rollout_offset))
            } else {
                None
            };
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
            let boundary = if let Some((fork_message_id, forked_through_at, source_offset)) =
                source_boundary
            {
                NativeForkBoundary {
                    fork_message_id,
                    forked_through_at,
                    source_rollout_offset: Some(source_offset),
                    branch_rollout_offset: Some(
                        capture_codex_rollout_offset(branch_session_id.clone()).await?,
                    ),
                    kind: "exact_rollout_offset".into(),
                }
            } else {
                NativeForkBoundary {
                    fork_message_id: None,
                    forked_through_at: chrono::Utc::now(),
                    source_rollout_offset: None,
                    branch_rollout_offset: None,
                    kind: "agent_timestamp_boundary".into(),
                }
            };
            Ok((branch_connection_id, branch_session_id, boundary))
        }
        .await;
        match native_attempt {
            Ok((connection_id, forked_session_id, boundary)) => {
                let inherited_message_count =
                    i32::try_from(source.message_count).unwrap_or(i32::MAX);
                let persisted = conversation_branch_service::create_branch_row_with_operation(
                    &db.conn,
                    &source,
                    creation_request_id.clone(),
                    operation_id.clone(),
                    Some(forked_session_id.clone()),
                    boundary.fork_message_id.clone(),
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
                            fork_message_id: boundary.fork_message_id.clone(),
                            forked_through_at: Some(boundary.forked_through_at),
                            snapshot_version: 2,
                            images: Vec::new(),
                        },
                        source.external_id.clone(),
                        Some(forked_session_id.clone()),
                        None,
                        boundary.source_rollout_offset,
                        boundary.branch_rollout_offset,
                        Some(boundary.kind.clone()),
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
                    creation_request_id = ?creation_request_id,
                    operation_id = ?operation_id,
                    source_conversation_id = source.id,
                    branch_conversation_id = branch.id,
                    source_session_id = source.external_id,
                    branch_session_id = forked_session_id,
                    fork_message_id = ?boundary.fork_message_id,
                    source_rollout_offset = ?boundary.source_rollout_offset,
                    branch_rollout_offset = ?boundary.branch_rollout_offset,
                    fork_boundary_kind = boundary.kind,
                    connection_id,
                    mapping_strategy = "immutable_source_detached_writer_handoff",
                    queue_state_before = "creating",
                    queue_state_after = "completed",
                    branch_lifecycle_state = "ready",
                    trigger = "native_fork_verified",
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
                if request.defer_if_source_busy
                    && (matches!(&error, AcpError::TurnInProgress)
                        || !artifact_service::active_runs_for_conversation(&db.conn, source.id)
                            .await?
                            .is_empty())
                {
                    tracing::info!(
                        creation_request_id = ?creation_request_id,
                        operation_id = ?operation_id,
                        source_conversation_id = source.id,
                        queue_state_before = "creating",
                        queue_state_after = "queued",
                        blocking_reason = "source_became_active_during_fork",
                        trigger = "native_fork_race_recheck",
                        error = %error,
                        "[ACP][branch-queue] source became active during fork; request deferred"
                    );
                    return Err(source_busy_branch_error());
                }
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
                .is_some_and(|v| !v.trim().is_empty())
                && request.fork_message_id.is_none() =>
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
        creation_request_id = ?creation_request_id,
        operation_id = ?operation_id,
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
        creation_request_id = ?creation_request_id,
        operation_id = ?operation_id,
        source_conversation_id = source.id,
        source_generating = source.status == "in_progress",
        lifecycle_state = "snapshot_ready",
        stage = "branch_snapshot_persist_started",
        fallback_reason = ?fallback_reason,
        "[ACP][branch] persisting provisional snapshot branch"
    );
    let snapshot = inheritance.context.clone();
    let (branch, relation) = conversation_branch_service::create_branch_row_with_operation(
        &db.conn,
        &source,
        creation_request_id.clone(),
        operation_id.clone(),
        None,
        inheritance.fork_message_id.clone(),
        "snapshot",
        inheritance_record(
            &inheritance,
            source.external_id.clone(),
            None,
            Some(snapshot),
            None,
            None,
            Some("snapshot_message_boundary".into()),
        ),
    )
    .await
    .map_err(AppCommandError::from)?;
    emit_conversation_upsert(emitter, &db.conn, branch.id).await;
    tracing::info!(
        creation_request_id = ?creation_request_id,
        operation_id = ?operation_id,
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
        queue_state_before = "creating",
        queue_state_after = "completed",
        branch_lifecycle_state = relation.lifecycle_state,
        trigger = "snapshot_persisted",
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

const BRANCH_MERGE_MAX_BYTES: u64 = 64 * 1024 * 1024;
const BRANCH_MERGE_MAX_TURNS: usize = 2_000;
const BRANCH_MERGE_PAGE_USER_TURNS: usize = 16;
const BRANCH_MERGE_PARSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25);
pub const BRANCH_MERGE_PROGRESS_EVENT: &str = "conversation-branch://merge-progress";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchMergeProgress<'a> {
    branch_conversation_id: i32,
    request_id: &'a str,
    stage: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

fn emit_branch_merge_progress(
    emitter: &EventEmitter,
    branch_conversation_id: i32,
    request_id: &str,
    stage: &str,
    error: Option<&str>,
) {
    emit_event(
        emitter,
        BRANCH_MERGE_PROGRESS_EVENT,
        BranchMergeProgress {
            branch_conversation_id,
            request_id,
            stage,
            error,
        },
    );
}

#[derive(Debug)]
struct BranchMergeIncrement {
    turns: Vec<crate::models::MessageTurn>,
    start_offset: Option<u64>,
    end_offset: Option<u64>,
    bytes_read: u64,
    boundary_kind: String,
}

fn merge_boundary_error(message: impl Into<String>) -> AppCommandError {
    AppCommandError::task_execution_failed(format!(
        "Branch merge could not determine a reliable fork boundary: {}. The branch was preserved; retry after repairing its boundary.",
        message.into()
    ))
}

async fn load_codex_merge_increment(
    relation: &conversation_branch_service::ConversationBranchInfo,
    branch_session_id: String,
    cwd: String,
) -> Result<BranchMergeIncrement, AppCommandError> {
    let relation = relation.clone();
    let task = tokio::task::spawn_blocking(move || {
        let parser = crate::parsers::codex::CodexParser::new();
        if let Some(saved_offset) = relation.branch_rollout_offset {
            let saved_offset = u64::try_from(saved_offset)
                .map_err(|_| merge_boundary_error("the saved branch rollout offset is negative"))?;
            let range = parser
                .get_conversation_range(
                    &branch_session_id,
                    saved_offset,
                    BRANCH_MERGE_MAX_BYTES,
                    Some(cwd),
                )
                .map_err(|error| merge_boundary_error(error.to_string()))?;
            let mut turns = range.detail.turns;
            if relation.fork_boundary_kind.as_deref() == Some("inferred_bounded_tail") {
                let boundary = relation.forked_through_at.ok_or_else(|| {
                    merge_boundary_error("the inferred boundary has no timestamp guard")
                })?;
                turns.retain(|turn| turn.timestamp > boundary);
            }
            if turns.len() > BRANCH_MERGE_MAX_TURNS {
                return Err(merge_boundary_error(format!(
                    "the branch delta contains {} visible turns, exceeding the safe limit of {BRANCH_MERGE_MAX_TURNS}",
                    turns.len()
                )));
            }
            return Ok(BranchMergeIncrement {
                bytes_read: range.end_offset.saturating_sub(range.start_offset),
                start_offset: Some(range.start_offset),
                end_offset: Some(range.end_offset),
                turns,
                boundary_kind: relation
                    .fork_boundary_kind
                    .unwrap_or_else(|| "saved_rollout_offset".into()),
            });
        }

        if relation.fork_mode != "native" {
            let file_len = parser
                .rollout_len(&branch_session_id)
                .map_err(|error| merge_boundary_error(error.to_string()))?;
            let range = parser
                .get_conversation_range(&branch_session_id, 0, BRANCH_MERGE_MAX_BYTES, Some(cwd))
                .map_err(|error| merge_boundary_error(error.to_string()))?;
            if range.detail.turns.len() > BRANCH_MERGE_MAX_TURNS {
                return Err(merge_boundary_error(format!(
                    "the snapshot branch contains {} visible turns, exceeding the safe limit of {BRANCH_MERGE_MAX_TURNS}",
                    range.detail.turns.len()
                )));
            }
            return Ok(BranchMergeIncrement {
                turns: range.detail.turns,
                start_offset: Some(0),
                end_offset: Some(file_len),
                bytes_read: file_len,
                boundary_kind: "snapshot_rollout_start".into(),
            });
        }

        let boundary = relation.forked_through_at.ok_or_else(|| {
            merge_boundary_error("legacy native fork has neither an offset nor forked_through_at")
        })?;
        let parent = crate::parsers::codex::native_fork_parent_session_id(&branch_session_id)
            .ok_or_else(|| {
                merge_boundary_error(
                    "the child rollout does not declare a native-fork parent session",
                )
            })?;
        if relation.source_session_id.as_deref() != Some(parent.as_str()) {
            return Err(merge_boundary_error(
                "the child rollout parent does not match the persisted source session",
            ));
        }

        // Legacy rows have no exact child EOF. Walk backwards in directly
        // seekable 16-user-turn pages until one crosses the creation boundary.
        // The scan stops at both a byte and visible-turn ceiling; it can recover
        // a two-round delta from a multi-gigabyte inherited rollout without
        // touching the inherited prefix.
        let mut before_offset = None;
        let mut scanned_bytes = 0u64;
        let mut scanned_turns = 0usize;
        let mut pages = Vec::new();
        let inferred_start = loop {
            let remaining = BRANCH_MERGE_MAX_BYTES.saturating_sub(scanned_bytes);
            if remaining == 0 || scanned_turns >= BRANCH_MERGE_MAX_TURNS {
                return Err(merge_boundary_error(format!(
                    "no timestamp seam was found within {scanned_bytes} bytes and {scanned_turns} visible turns"
                )));
            }
            let page = parser
                .get_conversation_page_bounded(
                    &branch_session_id,
                    before_offset,
                    BRANCH_MERGE_PAGE_USER_TURNS,
                    Some(cwd.clone()),
                    remaining,
                )
                .map_err(|error| merge_boundary_error(error.to_string()))?;
            let page_bytes = page.end_offset.saturating_sub(page.start_offset);
            scanned_bytes = scanned_bytes.saturating_add(page_bytes);
            scanned_turns = scanned_turns.saturating_add(page.detail.turns.len());
            let crosses_boundary = page
                .detail
                .turns
                .iter()
                .any(|turn| turn.timestamp <= boundary);
            let start_offset = page.start_offset;
            let has_more = page.has_more;
            pages.push(page.detail.turns);
            if crosses_boundary || !has_more {
                break start_offset;
            }
            before_offset = Some(start_offset);
        };
        pages.reverse();
        let mut turns = pages.into_iter().flatten().collect::<Vec<_>>();
        turns.retain(|turn| turn.timestamp > boundary);
        if turns.len() > BRANCH_MERGE_MAX_TURNS {
            return Err(merge_boundary_error(format!(
                "the inferred branch delta contains {} visible turns, exceeding the safe limit",
                turns.len()
            )));
        }
        let end_offset = parser
            .rollout_len(&branch_session_id)
            .map_err(|error| merge_boundary_error(error.to_string()))?;
        Ok(BranchMergeIncrement {
            turns,
            start_offset: Some(inferred_start),
            end_offset: Some(end_offset),
            bytes_read: scanned_bytes,
            boundary_kind: "inferred_bounded_tail".into(),
        })
    });
    tokio::time::timeout(BRANCH_MERGE_PARSE_TIMEOUT, task)
        .await
        .map_err(|_| {
            merge_boundary_error(format!(
                "bounded transcript extraction exceeded {} seconds",
                BRANCH_MERGE_PARSE_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|error| {
            merge_boundary_error(format!("bounded transcript task stopped: {error}"))
        })?
}

async fn load_branch_merge_increment(
    db: &AppDatabase,
    relation: &conversation_branch_service::ConversationBranchInfo,
) -> Result<BranchMergeIncrement, AppCommandError> {
    let branch = conversation_service::get_by_id(&db.conn, relation.branch_conversation_id)
        .await
        .map_err(AppCommandError::from)?;
    let folder = folder_service::get_folder_by_id(&db.conn, branch.folder_id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Branch folder was not found"))?;
    let session_id = relation
        .branch_session_id
        .as_deref()
        .or(branch.external_id.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let mut increment = if branch.agent_type == crate::models::AgentType::Codex {
        let session_id = session_id
            .ok_or_else(|| merge_boundary_error("the branch has no verified Codex session ID"))?;
        load_codex_merge_increment(relation, session_id, folder.path).await?
    } else {
        let (detail, _) = tokio::time::timeout(
            BRANCH_MERGE_PARSE_TIMEOUT,
            get_folder_conversation_raw_core(&db.conn, branch.id),
        )
        .await
        .map_err(|_| merge_boundary_error("branch transcript extraction timed out"))??;
        if detail.turns.len() > BRANCH_MERGE_MAX_TURNS {
            return Err(merge_boundary_error(format!(
                "the branch contains {} visible turns, exceeding the safe limit",
                detail.turns.len()
            )));
        }
        BranchMergeIncrement {
            turns: detail.turns,
            start_offset: None,
            end_offset: None,
            bytes_read: 0,
            boundary_kind: "agent_local_transcript".into(),
        }
    };
    if relation.fork_mode == "snapshot" {
        strip_branch_snapshot_context(&mut increment.turns);
    }
    strip_branch_merge_context(&mut increment.turns);
    Ok(increment)
}

async fn merge_conversation_branch_impl(
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
    let _merge_guard = lock_branch_merge(branch_conversation_id).await;
    if let Some(existing) = conversation_branch_service::existing_merge_result(
        &db.conn,
        branch_conversation_id,
        &request_id,
    )
    .await
    .map_err(AppCommandError::from)?
    {
        return Ok(existing);
    }
    let relation = conversation_branch_service::get_info(&db.conn, branch_conversation_id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Conversation branch was not found"))?;
    if relation.lifecycle_state != "ready" {
        return Err(AppCommandError::task_execution_failed(format!(
            "The branch is not ready to merge (state: {})",
            relation.lifecycle_state
        )));
    }
    emit_branch_merge_progress(
        emitter,
        branch_conversation_id,
        &request_id,
        "stopping_branch",
        None,
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

    emit_branch_merge_progress(
        emitter,
        branch_conversation_id,
        &request_id,
        "determining_boundary",
        None,
    );
    emit_branch_merge_progress(
        emitter,
        branch_conversation_id,
        &request_id,
        "extracting_increment",
        None,
    );
    let extraction_started = std::time::Instant::now();
    let increment = load_branch_merge_increment(db, &relation).await?;
    if relation.branch_rollout_offset.is_none()
        && increment.boundary_kind == "inferred_bounded_tail"
    {
        let inferred_offset = increment
            .start_offset
            .and_then(|offset| i64::try_from(offset).ok())
            .ok_or_else(|| merge_boundary_error("the inferred offset cannot be persisted"))?;
        conversation_branch_service::record_inferred_branch_boundary(
            &db.conn,
            branch_conversation_id,
            inferred_offset,
        )
        .await
        .map_err(AppCommandError::from)?;
    }
    tracing::info!(
        branch_conversation_id,
        source_conversation_id = relation.source_conversation_id,
        merge_request_id = request_id,
        fork_message_id = ?relation.fork_message_id,
        forked_through_at = ?relation.forked_through_at,
        start_offset = ?increment.start_offset,
        end_offset = ?increment.end_offset,
        bytes_read = increment.bytes_read,
        loaded_turns = increment.turns.len(),
        bounded_history = true,
        boundary_kind = increment.boundary_kind,
        elapsed_ms = extraction_started.elapsed().as_millis() as u64,
        stage = "branch_delta_extracted",
        "[ACP][branch] bounded merge increment extracted"
    );
    let branch = conversation_service::get_by_id(&db.conn, branch_conversation_id)
        .await
        .map_err(AppCommandError::from)?;
    let branch_title = branch.title.as_deref().unwrap_or("未命名分支");
    let mut sections = Vec::new();
    for turn in &increment.turns {
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
    let artifact_runs = artifact_service::list_for_conversation(&db.conn, branch_conversation_id)
        .await
        .map_err(AppCommandError::from)?;
    for run in &artifact_runs {
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
    emit_branch_merge_progress(
        emitter,
        branch_conversation_id,
        &request_id,
        "writing_source",
        None,
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

pub async fn merge_conversation_branch_core(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    branch_conversation_id: i32,
    request_id: String,
) -> Result<conversation_branch_service::MergeBranchResult, AppCommandError> {
    emit_branch_merge_progress(
        emitter,
        branch_conversation_id,
        &request_id,
        "started",
        None,
    );
    let result = merge_conversation_branch_impl(
        db,
        manager,
        emitter,
        branch_conversation_id,
        request_id.clone(),
    )
    .await;
    match &result {
        Ok(_) => emit_branch_merge_progress(
            emitter,
            branch_conversation_id,
            &request_id,
            "completed",
            None,
        ),
        Err(error) => {
            let message = error.to_string();
            emit_branch_merge_progress(
                emitter,
                branch_conversation_id,
                &request_id,
                "failed",
                Some(&message),
            );
            tracing::warn!(
                branch_conversation_id,
                merge_request_id = request_id,
                error = %message,
                stage = "branch_merge_failed",
                "[ACP][branch] one-click return to source failed without committing"
            );
        }
    }
    result
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
                request_id: Some("snapshot-first-request".into()),
                operation_id: Some("snapshot-stable-operation".into()),
                source_conversation_id: source_id,
                fork_message_id: None,
                snapshot_context: Some("User: context through message four".into()),
                defer_if_source_busy: false,
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

        let retry = create_conversation_branch_core(
            &db,
            &ConnectionManager::new(),
            &EventEmitter::Noop,
            Path::new("/tmp/codeg-message-branch-data"),
            "test".into(),
            CreateConversationBranchRequest {
                request_id: Some("snapshot-reconnected-request".into()),
                operation_id: Some("snapshot-stable-operation".into()),
                source_conversation_id: source_id,
                fork_message_id: None,
                snapshot_context: None,
                defer_if_source_busy: false,
                preferred_mode_id: Some("code".into()),
                preferred_config_values: BTreeMap::new(),
            },
        )
        .await
        .expect("a transport retry with the same operation must reuse the branch");
        assert_eq!(retry.branch_conversation_id, result.branch_conversation_id);
        assert_eq!(
            conversation_branch::Entity::find()
                .count(&db.conn)
                .await
                .unwrap(),
            1,
            "a successful operation terminates every later fallback/retry"
        );
    }

    #[tokio::test]
    async fn missing_fork_message_never_creates_a_half_ready_snapshot() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-missing-fork-message").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let error = create_conversation_branch_core(
            &db,
            &ConnectionManager::new(),
            &EventEmitter::Noop,
            Path::new("/tmp/codeg-missing-fork-message-data"),
            "test".into(),
            CreateConversationBranchRequest {
                request_id: Some("missing-message-request".into()),
                operation_id: Some("missing-message-operation".into()),
                source_conversation_id: source_id,
                fork_message_id: Some("turn-does-not-exist".into()),
                snapshot_context: Some("legacy browser context must not mask the error".into()),
                defer_if_source_busy: false,
                preferred_mode_id: None,
                preferred_config_values: BTreeMap::new(),
            },
        )
        .await
        .expect_err("an unknown durable fork point must be rejected");
        assert!(matches!(
            error.code,
            AppErrorCode::InvalidInput | AppErrorCode::NotFound | AppErrorCode::TaskExecutionFailed
        ));
        assert_eq!(
            conversation_branch::Entity::find()
                .count(&db.conn)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn pending_review_with_a_completed_settled_run_is_branch_idle() {
        let db = fresh_in_memory_db().await;
        let folder_path = "/tmp/codeg-terminal-source-branch";
        let folder_id = seed_folder(&db, folder_path).await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        artifact_service::create_run(
            &db.conn,
            artifact_service::NewTurnRun {
                id: "completed-source-turn".into(),
                conversation_id: source_id,
                connection_id: "old-terminal-connection".into(),
                client_message_id: Some("completed-source-message".into()),
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
        artifact_service::finish_run(
            &db.conn,
            "completed-source-turn",
            crate::db::entities::conversation_turn_run::ConversationTurnRunStatus::Completed,
            Some("end_turn".into()),
        )
        .await
        .unwrap();
        artifact_service::mark_settled(&db.conn, "completed-source-turn", "settled", &[])
            .await
            .unwrap();
        conversation_service::update_status(
            &db.conn,
            source_id,
            conversation::ConversationStatus::PendingReview,
        )
        .await
        .unwrap();
        assert_eq!(
            conversation_service::get_by_id(&db.conn, source_id)
                .await
                .unwrap()
                .status,
            "pending_review"
        );

        let result = create_conversation_branch_core(
            &db,
            &ConnectionManager::new(),
            &EventEmitter::Noop,
            Path::new("/tmp/codeg-terminal-source-branch-data"),
            "test".into(),
            CreateConversationBranchRequest {
                request_id: Some("terminal-source-request".into()),
                operation_id: Some("terminal-source-operation".into()),
                source_conversation_id: source_id,
                fork_message_id: None,
                snapshot_context: Some("completed context".into()),
                defer_if_source_busy: true,
                preferred_mode_id: None,
                preferred_config_values: BTreeMap::new(),
            },
        )
        .await
        .expect("pending_review is a stable idle state, not a branch blocker");
        assert_eq!(result.lifecycle_state, "provisional");
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

        let deferred = create_conversation_branch_core(
            &db,
            &ConnectionManager::new(),
            &EventEmitter::Noop,
            Path::new("/tmp/codeg-running-source-branch-data"),
            "test".into(),
            CreateConversationBranchRequest {
                request_id: Some("queued-running-source".into()),
                operation_id: Some("queued-running-source-operation".into()),
                source_conversation_id: source_id,
                fork_message_id: None,
                snapshot_context: Some("last committed context".into()),
                defer_if_source_busy: true,
                preferred_mode_id: None,
                preferred_config_values: BTreeMap::new(),
            },
        )
        .await
        .expect_err("queue-backed branch creation must wait for the active source turn");
        assert!(matches!(deferred.code, AppErrorCode::TurnInProgress));
        assert_eq!(
            conversation_branch::Entity::find()
                .count(&db.conn)
                .await
                .unwrap(),
            0,
            "a deferred request must not expose a half-created branch"
        );

        let result = create_conversation_branch_core(
            &db,
            &ConnectionManager::new(),
            &EventEmitter::Noop,
            Path::new("/tmp/codeg-running-source-branch-data"),
            "test".into(),
            CreateConversationBranchRequest {
                request_id: None,
                operation_id: None,
                source_conversation_id: source_id,
                fork_message_id: None,
                snapshot_context: Some("last committed context".into()),
                defer_if_source_busy: false,
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
