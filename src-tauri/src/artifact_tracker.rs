//! Backend-owned per-turn filesystem change capture.
//!
//! A capture starts before an ACP prompt is enqueued and owns a paths-only
//! workspace watcher lease until the turn reaches a terminal event. Debounced
//! semantic batches are UPSERTed continuously, so a process crash loses at most
//! the watcher debounce window rather than the entire turn.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, oneshot, Mutex};

use crate::db::entities::conversation_turn_file_change::ConversationTurnFileChangeKind;
use crate::db::entities::conversation_turn_run::ConversationTurnRunStatus;
use crate::db::service::artifact_service::{self, NewTurnRun, PendingFileChange};
use crate::db::service::deliverable_service;
use crate::web::event_bridge::{emit_event, EventEmitter};
use crate::workspace_state::{
    self, WorkspaceChangeSubscription, WorkspacePathChangeBatch, WorkspacePathChangeKind,
};

/// Extract real workspace file attachments from the structured prompt blocks.
/// This is exclusion metadata for conservative fallback inference only; it is
/// never used to *create* a deliverable. Plain assistant/user text is
/// intentionally ignored.
pub(crate) fn input_paths_from_prompt(
    blocks: &[crate::acp::types::PromptInputBlock],
    root_path: &Path,
) -> Vec<String> {
    fn file_uri_path(uri: &str) -> Option<PathBuf> {
        let raw = uri.strip_prefix("file://")?.split('#').next()?;
        let decoded = urlencoding::decode(raw).ok()?.into_owned();
        #[cfg(windows)]
        let decoded = {
            let bytes = decoded.as_bytes();
            if bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' {
                decoded[1..].to_string()
            } else if !decoded.starts_with('/') {
                format!("//{decoded}")
            } else {
                decoded
            }
        };
        Some(PathBuf::from(decoded))
    }

    let canonical_root = std::fs::canonicalize(root_path).unwrap_or_else(|_| root_path.into());
    let mut seen = std::collections::HashSet::new();
    let mut paths = Vec::new();
    for uri in blocks.iter().filter_map(|block| match block {
        crate::acp::types::PromptInputBlock::Image { uri, .. } => uri.as_deref(),
        crate::acp::types::PromptInputBlock::Resource { uri, .. }
        | crate::acp::types::PromptInputBlock::ResourceLink { uri, .. } => Some(uri.as_str()),
        crate::acp::types::PromptInputBlock::Text { .. } => None,
    }) {
        let Some(path) = file_uri_path(uri) else {
            continue;
        };
        let canonical = std::fs::canonicalize(&path).unwrap_or(path);
        let Ok(relative) = canonical.strip_prefix(&canonical_root) else {
            continue;
        };
        let normalized = normalize_relative_path(&relative.to_string_lossy());
        if !normalized.is_empty() && seen.insert(normalized.clone()) {
            paths.push(normalized);
        }
    }
    paths
}

fn fingerprint_field(hasher: &mut Sha256, tag: u8, value: &[u8]) {
    hasher.update([tag]);
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn finish_fingerprint(hasher: Sha256, block_count: usize) -> Option<String> {
    if block_count == 0 {
        return None;
    }
    Some(format!("{:x}", hasher.finalize()))
}

/// Stable, content-based bridge between an accepted prompt and the user turn
/// that agent parsers later reconstruct. Unlike `client_message_id`, this value
/// survives a reload on another machine; unlike timestamp matching, it cannot
/// attach an output card to a different prompt merely because the clocks were
/// close. Only the user-visible projection is hashed, so resource/link prompts
/// match the same representation broadcast to viewers.
pub(crate) fn prompt_fingerprint(blocks: &[crate::acp::types::PromptInputBlock]) -> Option<String> {
    let blocks = crate::acp::user_blocks_from_prompt(blocks);
    let mut hasher = Sha256::new();
    for block in &blocks {
        match block {
            crate::acp::types::UserMessageBlock::Text { text } => {
                fingerprint_field(&mut hasher, 1, text.as_bytes());
            }
            crate::acp::types::UserMessageBlock::Image { data, mime_type } => {
                fingerprint_field(&mut hasher, 2, mime_type.as_bytes());
                fingerprint_field(&mut hasher, 3, data.as_bytes());
            }
        }
    }
    finish_fingerprint(hasher, blocks.len())
}

/// Compute the same fingerprint from a parser-produced user turn. Any
/// non-user turn, or a user turn without text/image content, is intentionally
/// left unmatched and can still use the conservative timestamp fallback.
pub(crate) fn user_turn_fingerprint(turn: &crate::models::MessageTurn) -> Option<String> {
    if !matches!(turn.role, crate::models::TurnRole::User) {
        return None;
    }
    let mut hasher = Sha256::new();
    let mut count = 0;
    for block in &turn.blocks {
        match block {
            crate::models::ContentBlock::Text { text } => {
                fingerprint_field(&mut hasher, 1, text.as_bytes());
                count += 1;
            }
            crate::models::ContentBlock::Image {
                data, mime_type, ..
            } => {
                fingerprint_field(&mut hasher, 2, mime_type.as_bytes());
                fingerprint_field(&mut hasher, 3, data.as_bytes());
                count += 1;
            }
            _ => {}
        }
    }
    finish_fingerprint(hasher, count)
}

pub const CONVERSATION_ARTIFACTS_CHANGED_EVENT: &str = "conversation://artifacts-changed";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnDeliverableExpectation {
    pub publish_required: bool,
    pub expects_code_changes: bool,
    pub requested_paths: Vec<String>,
}

impl Default for TurnDeliverableExpectation {
    fn default() -> Self {
        Self {
            publish_required: true,
            expects_code_changes: false,
            requested_paths: Vec::new(),
        }
    }
}

fn prompt_text(blocks: &[crate::acp::types::PromptInputBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            crate::acp::types::PromptInputBlock::Text { text } => Some(text.trim()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn contains_bounded_ascii_marker(text: &str, marker: &str) -> bool {
    text.match_indices(marker).any(|(start, _)| {
        let end = start + marker.len();
        let left_boundary = start == 0
            || text[..start]
                .chars()
                .next_back()
                .is_some_and(|character| !character.is_ascii_alphanumeric());
        let right_boundary = end == text.len()
            || text[end..]
                .chars()
                .next()
                .is_some_and(|character| !character.is_ascii_alphanumeric());
        left_boundary && right_boundary
    })
}

fn expected_path_candidate(
    raw: &str,
    root_path: &Path,
    allow_extensionless: bool,
) -> Option<String> {
    let trimmed = raw.trim_matches(|character: char| {
        character.is_whitespace()
            || matches!(
                character,
                '`' | '"'
                    | '\''
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | ','
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '，'
                    | '。'
                    | '；'
                    | '：'
            )
    });
    let trimmed = trimmed.strip_suffix('.').unwrap_or(trimmed);
    if trimmed.is_empty()
        || trimmed.contains("://")
        || trimmed.starts_with('-')
        || trimmed.contains('\n')
    {
        return None;
    }
    let parsed = PathBuf::from(trimmed);
    if parsed.extension().is_none() && !allow_extensionless {
        return None;
    }
    let relative = if parsed.is_absolute() {
        parsed.strip_prefix(root_path).ok()?.to_path_buf()
    } else {
        parsed
    };
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }
    let normalized = normalize_relative_path(&relative.to_string_lossy());
    (!normalized.is_empty()).then_some(normalized)
}

/// Capture the user's output contract before the prompt is sent. Natural
/// language can only provide expectation candidates; settlement still requires
/// a verified declaration or a real filesystem change.
pub(crate) fn expectation_from_prompt(
    blocks: &[crate::acp::types::PromptInputBlock],
    root_path: &Path,
) -> TurnDeliverableExpectation {
    let text = prompt_text(blocks);
    let lower = text.to_lowercase();
    let expects_code_changes = [
        "implement",
        "fix",
        "modify",
        "edit",
        "update",
        "refactor",
        "add test",
        "change code",
        "add",
        "delete",
        "remove",
        "rename",
        "upgrade",
    ]
    .iter()
    .any(|marker| contains_bounded_ascii_marker(&lower, marker))
        || [
            "实现",
            "修复",
            "修改",
            "更新",
            "重构",
            "编写代码",
            "添加测试",
            "新增",
            "删除",
            "移除",
            "重命名",
            "调整",
        ]
        .iter()
        .any(|marker| lower.contains(marker));
    let has_output_intent = expects_code_changes
        || [
            "create", "generate", "export", "produce", "write", "save", "output", "deliver",
        ]
        .iter()
        .any(|marker| contains_bounded_ascii_marker(&lower, marker))
        || ["创建", "生成", "导出", "产出", "输出", "保存", "交付"]
            .iter()
            .any(|marker| lower.contains(marker));

    let mut candidates = Vec::new();
    let mut in_backticks = false;
    for segment in text.split('`') {
        if in_backticks {
            candidates.push((segment, true));
        }
        in_backticks = !in_backticks;
    }
    candidates.extend(text.split_whitespace().map(|candidate| (candidate, false)));

    let mut seen = std::collections::HashSet::new();
    let requested_paths = if has_output_intent {
        candidates
            .into_iter()
            .filter_map(|(candidate, allow_extensionless)| {
                expected_path_candidate(candidate, root_path, allow_extensionless)
            })
            .filter(|path| seen.insert(path.clone()))
            .collect()
    } else {
        Vec::new()
    };

    TurnDeliverableExpectation {
        publish_required: true,
        expects_code_changes,
        requested_paths,
    }
}

/// Wait past the workspace watcher's 300ms debounce when a turn completes, so
/// the final atomic rename/write reaches the persistent batch before releasing
/// the watcher lease.
const FINAL_EVENT_GRACE: Duration = Duration::from_millis(450);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactTurnFinishStatus {
    Completed,
    Cancelled,
    Interrupted,
    Failed,
}

impl ArtifactTurnFinishStatus {
    fn into_entity(self) -> ConversationTurnRunStatus {
        match self {
            Self::Completed => ConversationTurnRunStatus::Completed,
            Self::Cancelled => ConversationTurnRunStatus::Cancelled,
            Self::Interrupted => ConversationTurnRunStatus::Interrupted,
            Self::Failed => ConversationTurnRunStatus::Failed,
        }
    }
}

#[derive(Debug)]
struct FinishCommand {
    status: ArtifactTurnFinishStatus,
    stop_reason: Option<String>,
}

struct ActiveCapture {
    run_id: String,
    root_key: String,
    minimum_completion_seq: u64,
    ambiguous: Arc<AtomicBool>,
    finish_tx: oneshot::Sender<FinishCommand>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone, Default)]
pub struct ArtifactTracker {
    active: Arc<Mutex<HashMap<String, ActiveCapture>>>,
}

#[derive(Serialize)]
struct ConversationArtifactsChanged {
    conversation_id: i32,
    turn_run_id: String,
}

pub(crate) fn emit_artifacts_changed(
    emitter: &EventEmitter,
    conversation_id: i32,
    turn_run_id: String,
) {
    emit_event(
        emitter,
        CONVERSATION_ARTIFACTS_CHANGED_EVENT,
        ConversationArtifactsChanged {
            conversation_id,
            turn_run_id,
        },
    );
}

impl ArtifactTracker {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn begin_turn(
        &self,
        db: &DatabaseConnection,
        connection_id: &str,
        conversation_id: i32,
        client_message_id: Option<String>,
        prompt_fingerprint: Option<String>,
        folder_id: Option<i32>,
        root_path: PathBuf,
        input_paths: Vec<String>,
        expectation: TurnDeliverableExpectation,
        emitter: EventEmitter,
        event_seq_before_prompt: u64,
    ) -> Result<String, crate::db::error::DbError> {
        // A new prompt can be accepted before the lifecycle worker has drained
        // the preceding TurnComplete. Settle the stale capture now; sequence
        // gating below prevents that delayed completion from closing the new one.
        let previous = {
            let mut active = self.active.lock().await;
            active.remove(connection_id)
        };
        if let Some(previous) = previous {
            tracing::warn!(
                "[artifact-tracker] superseding unfinished capture {} on {}",
                previous.run_id,
                connection_id
            );
            settle_capture(
                previous,
                FinishCommand {
                    status: ArtifactTurnFinishStatus::Interrupted,
                    stop_reason: Some("superseded_before_lifecycle_finalize".to_string()),
                },
            )
            .await;
        }

        let requested_root = root_path.to_string_lossy().to_string();
        let subscription = match workspace_state::subscribe_workspace_changes(
            emitter.clone(),
            requested_root.clone(),
        )
        .await
        {
            Ok(subscription) => Some(subscription),
            Err(err) => {
                tracing::error!(
                    "[artifact-tracker] watcher unavailable for {}: {}",
                    requested_root,
                    err
                );
                None
            }
        };
        let stored_root = subscription
            .as_ref()
            .map(|sub| sub.root_path.clone())
            .unwrap_or_else(|| requested_root.clone());
        let capture_incomplete = subscription
            .as_ref()
            .map(|sub| sub.degraded)
            .unwrap_or(true);
        let run_id = uuid::Uuid::new_v4().to_string();

        if let Err(err) = artifact_service::create_run(
            db,
            NewTurnRun {
                id: run_id.clone(),
                conversation_id,
                connection_id: connection_id.to_string(),
                client_message_id,
                prompt_fingerprint,
                folder_id,
                root_path: stored_root.clone(),
                capture_incomplete,
                input_paths_json: serde_json::to_string(&input_paths)
                    .unwrap_or_else(|_| "[]".to_string()),
                expectation_json: serde_json::to_string(&expectation).unwrap_or_else(|_| {
                    r#"{"publish_required":true,"expects_code_changes":false,"requested_paths":[]}"#
                        .to_string()
                }),
            },
        )
        .await
        {
            if let Some(sub) = subscription {
                workspace_state::unsubscribe_workspace_changes(sub.root_path).await;
            }
            return Err(err);
        }

        let root_key = canonical_root_key(Path::new(&stored_root));
        let ambiguous = Arc::new(AtomicBool::new(false));
        let (finish_tx, finish_rx) = oneshot::channel();
        let task = tokio::spawn(capture_loop(CaptureLoopArgs {
            db: db.clone(),
            run_id: run_id.clone(),
            conversation_id,
            root_path: PathBuf::from(&stored_root),
            subscription,
            ambiguous: Arc::clone(&ambiguous),
            finish_rx,
            emitter,
        }));

        let overlapping = {
            let mut active = self.active.lock().await;
            let mut overlapping = Vec::new();
            for capture in active.values() {
                if capture.root_key == root_key {
                    capture.ambiguous.store(true, Ordering::Release);
                    overlapping.push(capture.run_id.clone());
                }
            }
            if !overlapping.is_empty() {
                ambiguous.store(true, Ordering::Release);
            }
            let replaced = active.insert(
                connection_id.to_string(),
                ActiveCapture {
                    run_id: run_id.clone(),
                    root_key,
                    minimum_completion_seq: event_seq_before_prompt.saturating_add(1),
                    ambiguous,
                    finish_tx,
                    task,
                },
            );
            debug_assert!(replaced.is_none());
            overlapping
        };

        for overlapping_run in overlapping {
            if let Err(err) = artifact_service::mark_run_ambiguous(db, &overlapping_run).await {
                tracing::error!(
                    "[artifact-tracker] failed to mark overlapping run {} ambiguous: {}",
                    overlapping_run,
                    err
                );
            }
        }

        tracing::info!(
            "[artifact-tracker] begin run={} conversation={} connection={} root={} incomplete={}",
            run_id,
            conversation_id,
            connection_id,
            stored_root,
            capture_incomplete
        );
        Ok(run_id)
    }

    /// Finish only if this terminal envelope belongs to the active generation.
    /// A delayed TurnComplete from the previous prompt has a lower sequence than
    /// the baseline captured before the new prompt and is intentionally ignored.
    pub async fn finish_turn(
        &self,
        connection_id: &str,
        completion_event_seq: u64,
        status: ArtifactTurnFinishStatus,
        stop_reason: Option<String>,
    ) {
        let capture = {
            let mut active = self.active.lock().await;
            let Some(capture) = active.get(connection_id) else {
                return;
            };
            if completion_event_seq < capture.minimum_completion_seq {
                tracing::debug!(
                    "[artifact-tracker] ignored stale terminal seq={} for run={} (minimum={})",
                    completion_event_seq,
                    capture.run_id,
                    capture.minimum_completion_seq
                );
                return;
            }
            active.remove(connection_id)
        };
        if let Some(capture) = capture {
            settle_capture(
                capture,
                FinishCommand {
                    status,
                    stop_reason,
                },
            )
            .await;
        }
    }

    /// Prompt enqueue failed after capture setup. There will be no ACP terminal
    /// envelope, so close the current generation directly.
    pub async fn cancel_unsent_turn(&self, connection_id: &str) {
        let capture = self.active.lock().await.remove(connection_id);
        if let Some(capture) = capture {
            settle_capture(
                capture,
                FinishCommand {
                    status: ArtifactTurnFinishStatus::Cancelled,
                    stop_reason: Some("prompt_send_failed".to_string()),
                },
            )
            .await;
        }
    }

    /// Force-close the capture owned by one connection without relying on an
    /// ACP event sequence. Used only after cancellation reaches its durable
    /// deadline or reconciliation proves the connection is gone. Removal from
    /// the map is the exactly-once gate shared with normal TurnComplete.
    pub async fn force_finish_turn(
        &self,
        connection_id: &str,
        status: ArtifactTurnFinishStatus,
        stop_reason: String,
    ) -> bool {
        let capture = self.active.lock().await.remove(connection_id);
        let Some(capture) = capture else {
            return false;
        };
        settle_capture(
            capture,
            FinishCommand {
                status,
                stop_reason: Some(stop_reason),
            },
        )
        .await;
        true
    }
}

async fn settle_capture(capture: ActiveCapture, command: FinishCommand) {
    let run_id = capture.run_id.clone();
    if capture.finish_tx.send(command).is_err() {
        tracing::error!(
            "[artifact-tracker] capture loop ended before run {} could be finalized",
            run_id
        );
    }
    if let Err(err) = capture.task.await {
        tracing::error!(
            "[artifact-tracker] capture task failed for run {}: {}",
            run_id,
            err
        );
    }
}

struct CaptureLoopArgs {
    db: DatabaseConnection,
    run_id: String,
    conversation_id: i32,
    root_path: PathBuf,
    subscription: Option<WorkspaceChangeSubscription>,
    ambiguous: Arc<AtomicBool>,
    finish_rx: oneshot::Receiver<FinishCommand>,
    emitter: EventEmitter,
}

async fn capture_loop(args: CaptureLoopArgs) {
    let CaptureLoopArgs {
        db,
        run_id,
        conversation_id,
        root_path,
        mut subscription,
        ambiguous,
        mut finish_rx,
        emitter,
    } = args;

    let finish = if let Some(sub) = subscription.as_mut() {
        let command_from_watch_loop = loop {
            tokio::select! {
                command = &mut finish_rx => {
                    break Some(command.unwrap_or(FinishCommand {
                        status: ArtifactTurnFinishStatus::Interrupted,
                        stop_reason: Some("capture_owner_dropped".to_string()),
                    }));
                }
                event = sub.receiver.recv() => {
                    match event {
                        Ok(batch) => persist_batch(&db, &run_id, &root_path, &ambiguous, batch).await,
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::error!(
                                "[artifact-tracker] run {} lagged by {} workspace batch(es)",
                                run_id,
                                skipped
                            );
                            let _ = artifact_service::mark_capture_incomplete(&db, &run_id).await;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            let _ = artifact_service::mark_capture_incomplete(&db, &run_id).await;
                            break None;
                        }
                    }
                }
            }
        };
        match command_from_watch_loop {
            Some(command) => command,
            None => finish_rx.await.unwrap_or(FinishCommand {
                status: ArtifactTurnFinishStatus::Interrupted,
                stop_reason: Some("workspace_watcher_closed".to_string()),
            }),
        }
    } else {
        finish_rx.await.unwrap_or(FinishCommand {
            status: ArtifactTurnFinishStatus::Interrupted,
            stop_reason: Some("capture_owner_dropped".to_string()),
        })
    };

    // Keep consuming during the final debounce window. The sender has already
    // observed TurnComplete, so new activity after this grace belongs to a
    // background task or later turn rather than the completed foreground turn.
    if let Some(sub) = subscription.as_mut() {
        let grace = tokio::time::sleep(FINAL_EVENT_GRACE);
        tokio::pin!(grace);
        loop {
            tokio::select! {
                _ = &mut grace => break,
                event = sub.receiver.recv() => {
                    match event {
                        Ok(batch) => persist_batch(&db, &run_id, &root_path, &ambiguous, batch).await,
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            let _ = artifact_service::mark_capture_incomplete(&db, &run_id).await;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    }

    if let Some(sub) = subscription.take() {
        workspace_state::unsubscribe_workspace_changes(sub.root_path).await;
    }
    let final_stats = finalize_paths(&db, &run_id, &root_path).await;
    if let Err(err) = artifact_service::finish_run(
        &db,
        &run_id,
        finish.status.into_entity(),
        finish.stop_reason.clone(),
    )
    .await
    {
        tracing::error!(
            "[artifact-tracker] failed to finalize run {}: {}",
            run_id,
            err
        );
    }

    // Only a normally completed turn may infer fallback deliverables. A
    // cancelled/interrupted capture can contain half-written files and must not
    // turn those into apparent successful output. Explicit declarations are
    // already durable and remain visible; we only close settlement here.
    if finish.status == ArtifactTurnFinishStatus::Completed {
        match deliverable_service::infer_for_turn(&db, conversation_id, &run_id).await {
            Ok(inferred) if !inferred.is_empty() => {
                crate::acp::deliverables::emit_deliverables_changed(
                    &emitter,
                    conversation_id,
                    inferred.into_iter().map(|item| item.id).collect(),
                );
            }
            Ok(_) => {}
            Err(err) => tracing::error!(
                "[artifact-tracker] fallback deliverable inference failed for run {}: {}",
                run_id,
                err
            ),
        }
    } else if let Err(err) =
        artifact_service::mark_settled(&db, &run_id, "settled_incomplete", &[]).await
    {
        tracing::error!(
            "[artifact-tracker] cancelled settlement failed for run {}: {}",
            run_id,
            err
        );
    }

    tracing::info!(
        "[artifact-tracker] finish run={} conversation={} status={:?} reason={} observed={} available={} removed={} stat_errors={}",
        run_id,
        conversation_id,
        finish.status,
        finish.stop_reason.as_deref().unwrap_or(""),
        final_stats.observed,
        final_stats.available,
        final_stats.removed,
        final_stats.stat_errors,
    );
    emit_artifacts_changed(&emitter, conversation_id, run_id);
}

async fn persist_batch(
    db: &DatabaseConnection,
    run_id: &str,
    root_path: &Path,
    ambiguous: &AtomicBool,
    batch: WorkspacePathChangeBatch,
) {
    if batch.overflowed {
        let _ = artifact_service::mark_capture_incomplete(db, run_id).await;
        return;
    }

    let attribution = if ambiguous.load(Ordering::Acquire) {
        "ambiguous"
    } else {
        "exclusive"
    };
    let changes = batch
        .changes
        .into_iter()
        .filter(|change| should_track_path(&change.path))
        .filter(|change| {
            // Existing directories are tree churn, not openable artifacts.
            // Removed directories cannot be identified here; finalization marks
            // them absent and the UI omits them with other deleted paths.
            !root_path.join(&change.path).is_dir()
        })
        .map(|change| PendingFileChange {
            path: normalize_relative_path(&change.path),
            kind: match change.kind {
                WorkspacePathChangeKind::Created => ConversationTurnFileChangeKind::Created,
                WorkspacePathChangeKind::Modified => ConversationTurnFileChangeKind::Modified,
                WorkspacePathChangeKind::Deleted => ConversationTurnFileChangeKind::Deleted,
            },
            attribution: attribution.to_string(),
        })
        .collect::<Vec<_>>();

    if let Err(err) = artifact_service::upsert_changes(db, run_id, changes).await {
        tracing::error!(
            "[artifact-tracker] failed to persist workspace batch for run {} (root={}): {}",
            run_id,
            batch.root_path,
            err
        );
        let _ = artifact_service::mark_capture_incomplete(db, run_id).await;
    }
}

#[derive(Default)]
struct FinalizeStats {
    observed: usize,
    available: usize,
    removed: usize,
    stat_errors: usize,
}

async fn finalize_paths(db: &DatabaseConnection, run_id: &str, root_path: &Path) -> FinalizeStats {
    let changes = match artifact_service::list_changes_for_run(db, run_id).await {
        Ok(changes) => changes,
        Err(err) => {
            tracing::error!(
                "[artifact-tracker] failed to list final paths for run {}: {}",
                run_id,
                err
            );
            let _ = artifact_service::mark_capture_incomplete(db, run_id).await;
            return FinalizeStats {
                stat_errors: 1,
                ..Default::default()
            };
        }
    };
    let mut stats = FinalizeStats {
        observed: changes.len(),
        ..Default::default()
    };

    for change in changes {
        let absolute = root_path.join(&change.path);
        match std::fs::metadata(&absolute) {
            Ok(metadata) if metadata.is_file() => {
                stats.available += 1;
                let size = i64::try_from(metadata.len()).ok();
                let modified_at = metadata.modified().ok().map(DateTime::<Utc>::from);
                if let Err(err) =
                    artifact_service::update_final_state(db, change, true, size, modified_at).await
                {
                    tracing::error!(
                        "[artifact-tracker] failed final file stat update for run {}: {}",
                        run_id,
                        err
                    );
                }
            }
            Ok(_) => {
                // Directories are never renderable/openable artifacts.
                let _ = artifact_service::delete_change(db, change).await;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                stats.removed += 1;
                let _ = artifact_service::update_final_state(db, change, false, None, None).await;
            }
            Err(err) => {
                stats.stat_errors += 1;
                tracing::error!(
                    "[artifact-tracker] final stat failed for {} (run={}): {}",
                    absolute.display(),
                    run_id,
                    err
                );
                let _ = artifact_service::mark_capture_incomplete(db, run_id).await;
            }
        }
    }
    stats
}

fn canonical_root_key(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let normalized = canonical.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    }
}

fn normalize_relative_path(path: &str) -> String {
    path.trim_start_matches(['/', '\\']).replace('\\', "/")
}

fn should_track_path(path: &str) -> bool {
    let normalized = normalize_relative_path(path);
    if normalized.is_empty() {
        return false;
    }
    let parsed = Path::new(&normalized);
    if parsed.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return false;
    }

    const IGNORED_DIRS: &[&str] = &[
        ".git",
        ".next",
        ".turbo",
        ".cache",
        ".venv",
        "venv",
        "node_modules",
        "target",
        "__pycache__",
    ];
    if parsed.components().any(|component| match component {
        Component::Normal(name) => IGNORED_DIRS
            .iter()
            .any(|ignored| name.to_string_lossy().eq_ignore_ascii_case(ignored)),
        _ => false,
    }) {
        return false;
    }

    let Some(name) = parsed.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if name.starts_with("~$")
        || lower == ".ds_store"
        || lower.ends_with(".swp")
        || lower.ends_with(".swo")
        || lower.ends_with(".tmp")
        || lower.starts_with(".~lock.")
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::types::PromptInputBlock;

    #[test]
    fn artifact_filter_drops_metadata_caches_and_office_lock_files() {
        assert!(!should_track_path(".git/index"));
        assert!(!should_track_path("node_modules/pkg/index.js"));
        assert!(!should_track_path("reports/~$quarterly.docx"));
        assert!(!should_track_path("../outside.txt"));
        assert!(should_track_path("Cargo.lock"));
        assert!(should_track_path("yarn.lock"));
        assert!(should_track_path("reports/quarterly.docx"));
        assert!(should_track_path("dist/release.zip"));
    }

    #[test]
    fn expectation_extracts_code_intent_and_explicit_paths_in_both_languages() {
        let root = Path::new("/workspace/project");
        let english = expectation_from_prompt(
            &[PromptInputBlock::Text {
                text: "Implement the fix in `src/lib.rs` and update tests/api.test.ts".into(),
            }],
            root,
        );
        assert!(english.publish_required);
        assert!(english.expects_code_changes);
        assert_eq!(
            english.requested_paths,
            vec!["src/lib.rs", "tests/api.test.ts"]
        );

        let chinese = expectation_from_prompt(
            &[PromptInputBlock::Text {
                text: "请实现这个功能，并生成 `docs/交付说明.md`。".into(),
            }],
            root,
        );
        assert!(chinese.expects_code_changes);
        assert_eq!(chinese.requested_paths, vec!["docs/交付说明.md"]);
    }

    #[test]
    fn expectation_does_not_treat_incidental_paths_as_an_output_contract() {
        let expectation = expectation_from_prompt(
            &[PromptInputBlock::Text {
                text: "What fixtures does src/lib.rs currently use?".into(),
            }],
            Path::new("/workspace/project"),
        );
        assert!(!expectation.expects_code_changes);
        assert!(expectation.requested_paths.is_empty());

        let prose = expectation_from_prompt(
            &[PromptInputBlock::Text {
                text: "Create a comparison of and/or semantics.".into(),
            }],
            Path::new("/workspace/project"),
        );
        assert!(prose.requested_paths.is_empty());
    }
}
