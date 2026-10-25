//! Final-delivery review copies. Never watch or guess a workspace draft.
use super::deliverable_service;
use crate::db::entities::{
    conversation_turn_run, conversation_turn_run::ConversationTurnRunStatus,
};
use crate::db::error::DbError;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, Statement, Value};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const COPY_BUDGET: Duration = Duration::from_secs(5);
const MAX_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfficeDeliveryPreview {
    pub conversation_id: i32,
    pub turn_run_id: String,
    pub run_sequence: i64,
    pub deliverable_id: Option<String>,
    pub file_name: Option<String>,
    pub original_path: Option<String>,
    pub snapshot_path: Option<String>,
    pub sha256: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub completed_at: String,
}

fn statement(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, sql, values)
}

pub async fn get(
    conn: &DatabaseConnection,
    cid: i32,
    run_id: &str,
) -> Result<Option<OfficeDeliveryPreview>, DbError> {
    let row = conn.query_one(statement("SELECT p.*, r.rowid AS run_sequence FROM office_delivery_preview p JOIN conversation_turn_run r ON r.id=p.turn_run_id JOIN conversation c ON c.id=p.conversation_id WHERE p.conversation_id=? AND p.turn_run_id=? AND c.deleted_at IS NULL AND r.status='completed' AND r.completed_at IS NOT NULL AND r.settlement_status<>'pending'", vec![cid.into(), run_id.into()])).await?;
    if let Some(row) = &row {
        if let Some(id) = row.try_get::<Option<String>>("", "deliverable_id")? {
            if !deliverable_service::has_trusted_turn_association(conn, cid, run_id, &id).await? {
                return Ok(None);
            }
        }
    }
    row.map(|row| {
        Ok(OfficeDeliveryPreview {
            conversation_id: row.try_get("", "conversation_id")?,
            turn_run_id: row.try_get("", "turn_run_id")?,
            run_sequence: row.try_get("", "run_sequence")?,
            deliverable_id: row.try_get("", "deliverable_id")?,
            file_name: row.try_get("", "file_name")?,
            original_path: row.try_get("", "original_path")?,
            snapshot_path: row.try_get("", "snapshot_path")?,
            sha256: row.try_get("", "sha256")?,
            status: row.try_get("", "status")?,
            error: row.try_get("", "error")?,
            completed_at: row.try_get("", "completed_at")?,
        })
    })
    .transpose()
}

fn office_file(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| {
        matches!(
            ext.to_string_lossy().to_ascii_lowercase().as_str(),
            "docx" | "xlsx" | "pptx"
        )
    })
}

/// Copy bytes before settlement admits the next queued turn. Conversion is
/// lazy and is never part of the task's lifecycle. Work is bounded by size and
/// a cooperative deadline, and source changes during copy are rejected.
pub async fn capture(conn: &DatabaseConnection, cid: i32, run_id: &str) -> Result<(), DbError> {
    let Some(run) = conversation_turn_run::Entity::find_by_id(run_id)
        .one(conn)
        .await?
    else {
        return Ok(());
    };
    if run.conversation_id != cid || run.status != ConversationTurnRunStatus::Completed {
        return Ok(());
    }
    let Some(completed_at) = run.completed_at else {
        return Ok(());
    };
    if conn
        .query_one(statement(
            "SELECT status FROM office_delivery_preview WHERE turn_run_id=? AND status<>'none'",
            vec![run_id.into()],
        ))
        .await?
        .is_some()
    {
        return Ok(());
    }
    let mut files = deliverable_service::list_for_turn(conn, cid, run_id).await?;
    files.retain(|file| {
        file.is_valid
            && file.kind == "file"
            && file.change_kind != "deleted"
            && office_file(&file.path)
    });
    files.sort_by(|a, b| {
        (a.role != "primary", a.position, &a.path, &a.id).cmp(&(
            b.role != "primary",
            b.position,
            &b.path,
            &b.id,
        ))
    });
    let selected = files.first();
    let mut preview = OfficeDeliveryPreview {
        conversation_id: cid,
        turn_run_id: run_id.into(),
        run_sequence: 0,
        deliverable_id: selected.map(|f| f.id.clone()),
        file_name: selected.map(|f| f.file_name.clone()),
        original_path: None,
        snapshot_path: None,
        sha256: None,
        status: "none".into(),
        error: None,
        completed_at: completed_at.to_rfc3339(),
    };
    let mut retained = None;
    if let Some(file) = selected {
        let resolved =
            deliverable_service::resolve_for_access(conn, cid, std::slice::from_ref(&file.id))
                .await;
        let result = match resolved {
            Ok(resolved) => {
                let source = resolved[0].absolute_path.clone();
                preview.original_path = Some(source.to_string_lossy().into_owned());
                let db_files = conn
                    .query_all(statement("PRAGMA database_list", vec![]))
                    .await?;
                let db_path = db_files
                    .iter()
                    .filter_map(|row| row.try_get::<String>("", "file").ok())
                    .find(|path| !path.is_empty());
                let cache = db_path
                    .and_then(|path| {
                        PathBuf::from(path)
                            .parent()
                            .map(|p| p.join("office-preview-cache"))
                    })
                    .unwrap_or_else(|| std::env::temp_dir().join("codeg-office-delivery-cache"));
                // A completed historical run cannot snapshot today's overwritten
                // source as though it were its old delivery. Late reports are
                // eligible only before another turn in this workspace starts.
                let newer = conn.query_one(statement(
                    "SELECT 1 FROM conversation_turn_run WHERE root_path=? AND started_at>? AND id<>? LIMIT 1",
                    vec![run.root_path.clone().into(), run.started_at.into(), run.id.clone().into()],
                )).await?.is_some();
                if newer {
                    Err(std::io::Error::other("The delivery was not captured before a later task started; use the current-file action instead"))
                } else {
                    freeze_file_bounded(source, cache, COPY_BUDGET).await
                }
            }
            Err(err) => Err(std::io::Error::other(err.to_string())),
        };
        match result {
            Ok((directory, path, hash)) => {
                preview.status = "ready".into();
                preview.snapshot_path = Some(path.to_string_lossy().into());
                preview.sha256 = Some(hash);
                retained = Some(directory);
            }
            Err(err) => {
                preview.status = "failed".into();
                preview.error = Some(err.to_string());
            }
        }
    }
    let changed = conn.execute(statement("INSERT INTO office_delivery_preview(turn_run_id,conversation_id,deliverable_id,file_name,original_path,snapshot_path,sha256,status,error,completed_at) VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT(turn_run_id) DO UPDATE SET deliverable_id=excluded.deliverable_id,file_name=excluded.file_name,original_path=excluded.original_path,snapshot_path=excluded.snapshot_path,sha256=excluded.sha256,status=excluded.status,error=excluded.error WHERE office_delivery_preview.status='none'", vec![run_id.into(),cid.into(),preview.deliverable_id.into(),preview.file_name.into(),preview.original_path.into(),preview.snapshot_path.into(),preview.sha256.into(),preview.status.into(),preview.error.into(),preview.completed_at.into()])).await?;
    if changed.rows_affected() > 0 {
        if let Some(directory) = retained {
            let _ = directory.keep();
        }
    }
    Ok(())
}

/// An outer deadline also bounds blocked filesystem I/O. A timed-out worker
/// owns only a TempDir; its eventual result is dropped rather than retained.
pub(crate) async fn freeze_file_bounded(
    source: PathBuf,
    cache: PathBuf,
    budget: Duration,
) -> std::io::Result<(tempfile::TempDir, PathBuf, String)> {
    match tokio::time::timeout(
        budget,
        tokio::task::spawn_blocking(move || freeze_file(&source, &cache)),
    )
    .await
    {
        Ok(result) => result.map_err(|error| std::io::Error::other(error.to_string()))?,
        Err(_) => Err(std::io::Error::other("Office review copy timed out")),
    }
}

pub(crate) fn freeze_file(
    source: &Path,
    cache: &Path,
) -> std::io::Result<(tempfile::TempDir, PathBuf, String)> {
    std::fs::create_dir_all(cache)?;
    let directory = tempfile::Builder::new()
        .prefix("review-")
        .tempdir_in(cache)?;
    let path = directory.path().join(
        source
            .file_name()
            .ok_or_else(|| std::io::Error::other("missing file name"))?,
    );
    let mut input = std::fs::File::open(source)?;
    let before = input.metadata()?;
    if !before.is_file() || before.len() > MAX_BYTES {
        return Err(std::io::Error::other(
            "Office review copy exceeds 128 MiB or is not a file",
        ));
    }
    let deadline = Instant::now() + COPY_BUDGET;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut hash = Sha256::new();
    let mut copied = 0_u64;
    loop {
        if Instant::now() > deadline {
            return Err(std::io::Error::other("Office review copy timed out"));
        }
        let bytes = input.read(&mut buffer)?;
        if bytes == 0 {
            break;
        }
        copied += bytes as u64;
        if copied > MAX_BYTES {
            return Err(std::io::Error::other("Office review copy exceeds 128 MiB"));
        }
        output.write_all(&buffer[..bytes])?;
        hash.update(&buffer[..bytes]);
    }
    let after = input.metadata()?;
    if copied != before.len()
        || after.len() != before.len()
        || after.modified()? != before.modified()?
    {
        return Err(std::io::Error::other(
            "Office file changed while preparing its review copy",
        ));
    }
    Ok((directory, path, format!("{:x}", hash.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        service::{
            artifact_service,
            deliverable_service::{self, VerifiedDeliverable},
        },
        test_helpers,
    };
    use crate::models::AgentType;

    struct Fixture {
        _dir: tempfile::TempDir,
        db: crate::db::AppDatabase,
        root: PathBuf,
        cid: i32,
        folder: i32,
    }
    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("workspace");
            std::fs::create_dir(&root).unwrap();
            let db = test_helpers::fresh_disk_db(dir.path()).await;
            let folder = test_helpers::seed_folder(&db, &root.to_string_lossy()).await;
            let cid = test_helpers::seed_conversation(&db, folder, AgentType::Pi).await;
            Self {
                _dir: dir,
                db,
                root,
                cid,
                folder,
            }
        }
        async fn run(&self, id: &str) {
            self.run_for(self.cid, id).await;
        }
        async fn run_for(&self, cid: i32, id: &str) {
            artifact_service::create_run(
                &self.db.conn,
                artifact_service::NewTurnRun {
                    id: id.into(),
                    conversation_id: cid,
                    connection_id: format!("conn-{id}"),
                    client_message_id: Some(format!("msg-{id}")),
                    prompt_fingerprint: None,
                    folder_id: Some(self.folder),
                    root_path: self.root.to_string_lossy().into(),
                    capture_incomplete: false,
                    input_paths_json: "[]".into(),
                    expectation_json: "{}".into(),
                },
            )
            .await
            .unwrap();
        }
        fn file(&self, path: &str, text: &str, role: &str) -> VerifiedDeliverable {
            std::fs::write(self.root.join(path), text).unwrap();
            VerifiedDeliverable {
                root_path: self.root.to_string_lossy().into(),
                path: path.into(),
                kind: "file".into(),
                title: path.into(),
                description: None,
                role: role.into(),
                category: "standalone_output".into(),
                change_kind: "created".into(),
                file_name: path.into(),
                extension: Path::new(path)
                    .extension()
                    .map(|s| s.to_string_lossy().into()),
                size_bytes: Some(text.len() as i64),
                modified_at: None,
                is_valid: true,
                invalid_reason: None,
            }
        }
        async fn declare(&self, run: &str, files: Vec<VerifiedDeliverable>) {
            deliverable_service::replace_declared_for_turn(&self.db.conn, self.cid, run, files)
                .await
                .unwrap();
        }
        async fn observe(&self, run: &str, file: &VerifiedDeliverable, attribution: &str) {
            artifact_service::upsert_changes(
                &self.db.conn,
                run,
                vec![artifact_service::PendingFileChange {
                    path: file.path.clone(),
                    kind: crate::db::entities::conversation_turn_file_change::ConversationTurnFileChangeKind::Created,
                    attribution: attribution.into(),
                }],
            )
            .await
            .unwrap();
            let change = artifact_service::list_changes_for_run(&self.db.conn, run)
                .await
                .unwrap()
                .pop()
                .unwrap();
            artifact_service::update_final_state(
                &self.db.conn,
                change,
                true,
                file.size_bytes,
                None,
            )
            .await
            .unwrap();
        }
        async fn finish(&self, run: &str) -> OfficeDeliveryPreview {
            artifact_service::finish_run(
                &self.db.conn,
                run,
                ConversationTurnRunStatus::Completed,
                None,
            )
            .await
            .unwrap();
            deliverable_service::infer_for_turn(&self.db.conn, self.cid, run)
                .await
                .unwrap();
            get(&self.db.conn, self.cid, run).await.unwrap().unwrap()
        }
    }

    #[tokio::test]
    async fn ambiguous_workspace_output_is_not_a_turn_delivery_or_office_review() {
        let f = Fixture::new().await;
        let other = test_helpers::seed_conversation(&f.db, f.folder, AgentType::Codex).await;
        // The other business started first. A newer-task-only snapshot check
        // cannot establish who produced a file while both tasks are running.
        f.run_for(other, "other-business").await;
        f.run("current-business").await;
        let file = f.file("report.docx", "other business output", "supporting");
        f.observe("current-business", &file, "ambiguous").await;

        let preview = f.finish("current-business").await;
        assert_eq!(preview.status, "none", "An ambiguous file cannot auto-open");
        assert!(
            deliverable_service::list_for_turn(&f.db.conn, f.cid, "current-business")
                .await
                .unwrap()
                .is_empty()
        );
        let ledger = deliverable_service::list_for_conversation(&f.db.conn, f.cid)
            .await
            .unwrap();
        assert_eq!(
            ledger.len(),
            1,
            "Retain the observed file in the total list"
        );
        assert!(
            ledger[0].turn_run_id.is_none(),
            "Do not invent a turn owner"
        );
        assert_eq!(ledger[0].role, "supporting");
        assert!(
            deliverable_service::list_sets_for_conversation(&f.db.conn, f.cid)
                .await
                .unwrap()
                .is_empty()
        );
        let page = deliverable_service::list_history_page(&f.db.conn, f.cid, 0, 1)
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert!(page.items[0].latest.turn_run_id.is_none());
        deliverable_service::infer_for_turn(&f.db.conn, f.cid, "current-business")
            .await
            .unwrap();
        let repeated = deliverable_service::list_for_conversation(&f.db.conn, f.cid)
            .await
            .unwrap();
        assert_eq!(repeated.len(), 1);
        assert_eq!(repeated[0].id, ledger[0].id);
        assert!(repeated[0].turn_run_id.is_none());
    }

    #[tokio::test]
    async fn exclusive_inference_and_explicit_claims_still_produce_review_copies() {
        for declared in [false, true] {
            let f = Fixture::new().await;
            f.run("owned-output").await;
            let file = f.file("review.docx", "owned delivery", "primary");
            f.observe(
                "owned-output",
                &file,
                if declared { "ambiguous" } else { "exclusive" },
            )
            .await;
            if declared {
                f.declare("owned-output", vec![file]).await;
            }
            let preview = f.finish("owned-output").await;
            assert_eq!(preview.status, "ready");
            assert_eq!(
                std::fs::read_to_string(preview.snapshot_path.unwrap()).unwrap(),
                "owned delivery"
            );
            assert_eq!(
                deliverable_service::list_for_turn(&f.db.conn, f.cid, "owned-output")
                    .await
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    #[tokio::test]
    async fn legacy_ambiguous_association_is_read_as_unlinked_without_deleting_evidence() {
        let f = Fixture::new().await;
        f.run("legacy").await;
        let file = f.file("legacy.docx", "legacy bytes", "primary");
        f.declare("legacy", vec![file.clone()]).await;
        let preview = f.finish("legacy").await;
        assert_eq!(preview.status, "ready");
        // Recreate the old persisted inference shape, including its preview.
        for sql in [
            "UPDATE conversation_deliverable SET source='inferred'",
            "UPDATE conversation_turn_deliverable SET source='inferred'",
        ] {
            f.db.conn.execute(statement(sql, vec![])).await.unwrap();
        }
        f.observe("legacy", &file, "ambiguous").await;
        assert!(get(&f.db.conn, f.cid, "legacy").await.unwrap().is_none());
        assert!(
            deliverable_service::list_for_turn(&f.db.conn, f.cid, "legacy")
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            deliverable_service::list_sets_for_conversation(&f.db.conn, f.cid)
                .await
                .unwrap()
                .is_empty()
        );
        let total = deliverable_service::list_history_page(&f.db.conn, f.cid, 0, 25)
            .await
            .unwrap();
        assert_eq!(total.total, 1);
        assert_eq!(total.items[0].versions.len(), 1);
        assert!(total.items[0].latest.turn_run_id.is_none());
        assert_eq!(total.items[0].latest.role, "supporting");
        let evidence =
            f.db.conn
                .query_one(statement(
                    "SELECT turn_run_id FROM conversation_turn_deliverable",
                    vec![],
                ))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            evidence.try_get::<String>("", "turn_run_id").unwrap(),
            "legacy"
        );
        assert_eq!(
            std::fs::read_to_string(preview.snapshot_path.unwrap()).unwrap(),
            "legacy bytes"
        );
    }

    #[tokio::test]
    async fn ambiguous_later_observation_cannot_overwrite_a_declared_files_provenance() {
        let f = Fixture::new().await;
        f.run("declared").await;
        f.declare(
            "declared",
            vec![f.file("report.docx", "delivered v1", "primary")],
        )
        .await;
        let first = f.finish("declared").await;
        f.run("ambiguous-later").await;
        let file = f.file("report.docx", "unknown workspace writer", "supporting");
        f.observe("ambiguous-later", &file, "ambiguous").await;
        assert_eq!(f.finish("ambiguous-later").await.status, "none");
        let total = deliverable_service::list_history_page(&f.db.conn, f.cid, 0, 25)
            .await
            .unwrap();
        assert_eq!(total.total, 1);
        assert_eq!(total.items[0].versions.len(), 1);
        assert_eq!(
            total.items[0].latest.turn_run_id.as_deref(),
            Some("declared")
        );
        assert_eq!(total.items[0].latest.source, "declared");
        assert_eq!(total.items[0].latest.role, "primary");
        std::fs::remove_file(f.root.join("report.docx")).unwrap();
        assert!(get(&f.db.conn, f.cid, "declared").await.unwrap().is_some());
        assert_eq!(
            std::fs::read_to_string(first.snapshot_path.unwrap()).unwrap(),
            "delivered v1"
        );
    }

    #[tokio::test]
    async fn only_final_declared_set_is_captured_after_settlement() {
        let f = Fixture::new().await;
        f.run("final-set").await;
        f.declare(
            "final-set",
            vec![f.file("process.docx", "draft", "primary")],
        )
        .await;
        capture(&f.db.conn, f.cid, "final-set").await.unwrap();
        assert!(get(&f.db.conn, f.cid, "final-set").await.unwrap().is_none());
        f.declare(
            "final-set",
            vec![
                f.file("support.xlsx", "support", "supporting"),
                f.file("review.pptx", "delivered", "primary"),
            ],
        )
        .await;
        let review = f.finish("final-set").await;
        assert_eq!(review.status, "ready");
        assert_eq!(review.file_name.as_deref(), Some("review.pptx"));
        assert_eq!(
            std::fs::read_to_string(review.snapshot_path.unwrap()).unwrap(),
            "delivered"
        );
        assert!(get(&f.db.conn, f.cid + 1, "final-set")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn explicit_withdrawal_and_no_office_output_do_not_open_a_draft() {
        let f = Fixture::new().await;
        f.run("withdrawn").await;
        f.declare("withdrawn", vec![f.file("draft.docx", "draft", "primary")])
            .await;
        f.declare("withdrawn", vec![]).await;
        assert_eq!(f.finish("withdrawn").await.status, "none");
        f.run("pdf-only").await;
        f.declare("pdf-only", vec![f.file("report.pdf", "pdf", "primary")])
            .await;
        assert_eq!(f.finish("pdf-only").await.status, "none");
    }

    #[tokio::test]
    async fn failed_and_cancelled_runs_never_capture_but_keep_manual_outputs() {
        let f = Fixture::new().await;
        for (run, status) in [
            ("failed", ConversationTurnRunStatus::Failed),
            ("cancelled", ConversationTurnRunStatus::Cancelled),
        ] {
            f.run(run).await;
            f.declare(run, vec![f.file("review.docx", "partial", "primary")])
                .await;
            artifact_service::finish_run(&f.db.conn, run, status, None)
                .await
                .unwrap();
            capture(&f.db.conn, f.cid, run).await.unwrap();
            artifact_service::mark_settled(&f.db.conn, run, "settled_incomplete", &[])
                .await
                .unwrap();
            assert!(get(&f.db.conn, f.cid, run).await.unwrap().is_none());
            assert_eq!(
                deliverable_service::list_for_turn(&f.db.conn, f.cid, run)
                    .await
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    #[tokio::test]
    async fn same_path_next_turn_and_new_path_keep_previous_delivered_bytes_stable() {
        let f = Fixture::new().await;
        f.run("version-one").await;
        f.declare("version-one", vec![f.file("report.docx", "v1", "primary")])
            .await;
        let first = f.finish("version-one").await;
        f.run("version-two").await;
        let second_file = f.file("report.docx", "half-finished", "primary");
        assert_eq!(
            std::fs::read_to_string(first.snapshot_path.as_ref().unwrap()).unwrap(),
            "v1"
        );
        std::fs::write(f.root.join("report.docx"), "v2").unwrap();
        f.declare("version-two", vec![second_file]).await;
        let second = f.finish("version-two").await;
        assert_eq!(
            std::fs::read_to_string(second.snapshot_path.as_ref().unwrap()).unwrap(),
            "v2"
        );
        assert_ne!(first.snapshot_path, second.snapshot_path);
        assert!(second.run_sequence > first.run_sequence);
        f.run("version-three").await;
        f.declare(
            "version-three",
            vec![f.file("renamed.docx", "v3", "primary")],
        )
        .await;
        let third = f.finish("version-three").await;
        assert!(third.original_path.unwrap().ends_with("renamed.docx"));
        assert_eq!(
            std::fs::read_to_string(first.snapshot_path.unwrap()).unwrap(),
            "v1"
        );
    }

    #[tokio::test]
    async fn no_primary_selects_one_stable_file_and_duplicate_events_do_not_recapture() {
        let f = Fixture::new().await;
        f.run("multi").await;
        f.declare(
            "multi",
            vec![
                f.file("first.xlsx", "one", "supporting"),
                f.file("second.pptx", "two", "supporting"),
            ],
        )
        .await;
        let first = f.finish("multi").await;
        assert_eq!(first.file_name.as_deref(), Some("first.xlsx"));
        std::fs::write(f.root.join("first.xlsx"), "later").unwrap();
        capture(&f.db.conn, f.cid, "multi").await.unwrap();
        let again = get(&f.db.conn, f.cid, "multi").await.unwrap().unwrap();
        assert_eq!(again.snapshot_path, first.snapshot_path);
        assert_eq!(
            std::fs::read_to_string(again.snapshot_path.unwrap()).unwrap(),
            "one"
        );
    }

    #[tokio::test]
    async fn missing_historical_capture_never_reads_a_later_overwrite_as_old_delivery() {
        let f = Fixture::new().await;
        f.run("old").await;
        f.declare("old", vec![f.file("report.docx", "v1", "primary")])
            .await;
        artifact_service::finish_run(
            &f.db.conn,
            "old",
            ConversationTurnRunStatus::Completed,
            None,
        )
        .await
        .unwrap();
        f.run("new").await;
        std::fs::write(f.root.join("report.docx"), "v2-draft").unwrap();
        deliverable_service::infer_for_turn(&f.db.conn, f.cid, "old")
            .await
            .unwrap();
        let review = get(&f.db.conn, f.cid, "old").await.unwrap().unwrap();
        assert_eq!(review.status, "failed");
        assert!(review.snapshot_path.is_none());
    }

    #[tokio::test]
    async fn oversized_review_fails_without_blocking_task_settlement() {
        let f = Fixture::new().await;
        f.run("large").await;
        f.declare("large", vec![f.file("large.docx", "start", "primary")])
            .await;
        std::fs::OpenOptions::new()
            .write(true)
            .open(f.root.join("large.docx"))
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        let preview = f.finish("large").await;
        assert_eq!(preview.status, "failed");
        assert!(preview.error.unwrap().contains("128 MiB"));
        assert!(preview.snapshot_path.is_none());
    }

    #[tokio::test]
    async fn manual_review_copy_is_independent_of_source_writes() {
        let source = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = source.path().join("manual.docx");
        std::fs::write(&path, "opened").unwrap();
        let (_copy, frozen, hash) =
            freeze_file_bounded(path.clone(), cache.path().into(), COPY_BUDGET)
                .await
                .unwrap();
        std::fs::write(path, "process draft").unwrap();
        assert_eq!(std::fs::read(frozen).unwrap(), b"opened");
        assert_eq!(hash.len(), 64);
    }
}
