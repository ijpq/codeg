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
use serde::Serialize;
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

pub const CONVERSATION_ARTIFACTS_CHANGED_EVENT: &str = "conversation://artifacts-changed";

/// Wait past the workspace watcher's 300ms debounce when a turn completes, so
/// the final atomic rename/write reaches the persistent batch before releasing
/// the watcher lease.
const FINAL_EVENT_GRACE: Duration = Duration::from_millis(450);

#[derive(Debug, Clone, Copy)]
pub enum ArtifactTurnFinishStatus {
    Completed,
    Cancelled,
    Interrupted,
}

impl ArtifactTurnFinishStatus {
    fn into_entity(self) -> ConversationTurnRunStatus {
        match self {
            Self::Completed => ConversationTurnRunStatus::Completed,
            Self::Cancelled => ConversationTurnRunStatus::Cancelled,
            Self::Interrupted => ConversationTurnRunStatus::Interrupted,
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
        folder_id: Option<i32>,
        root_path: PathBuf,
        input_paths: Vec<String>,
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
                folder_id,
                root_path: stored_root.clone(),
                capture_incomplete,
                input_paths_json: serde_json::to_string(&input_paths)
                    .unwrap_or_else(|_| "[]".to_string()),
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

    // The declaration marker is persisted while the turn is running. The
    // fallback therefore cannot overwrite an explicit set (including an
    // explicit empty set) once the terminal event arrives.
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
    emit_event(
        &emitter,
        CONVERSATION_ARTIFACTS_CHANGED_EVENT,
        ConversationArtifactsChanged {
            conversation_id,
            turn_run_id: run_id,
        },
    );
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
        || lower.ends_with(".lock")
        || lower.starts_with(".~lock.")
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_filter_drops_metadata_caches_and_office_lock_files() {
        assert!(!should_track_path(".git/index"));
        assert!(!should_track_path("node_modules/pkg/index.js"));
        assert!(!should_track_path("reports/~$quarterly.docx"));
        assert!(!should_track_path("../outside.txt"));
        assert!(should_track_path("reports/quarterly.docx"));
        assert!(should_track_path("dist/release.zip"));
    }
}
