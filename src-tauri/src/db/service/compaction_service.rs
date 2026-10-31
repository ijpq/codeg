//! Durable display-only Pi compaction lifecycle. Native JSONL stays untouched.
//! These records do not claim a task duration, generated reply or native summary.
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, Value};
use serde_json::{json, Value as Json};

use crate::acp::{
    compaction::{lifecycle_meta, BINDING_KEY},
    types::{AcpEvent, EventEnvelope},
};
use crate::db::error::DbError;
use crate::models::{ContentBlock, MessageTurn, TurnRole};

fn stmt(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, sql, values)
}

pub(crate) async fn persist(
    conn: &DatabaseConnection,
    envelope: &EventEnvelope,
) -> Result<(), DbError> {
    let Some(meta) = lifecycle_meta(&envelope.payload) else {
        return Ok(());
    };
    let owner = &meta[BINDING_KEY];
    let cid = owner["conversationId"]
        .as_i64()
        .ok_or_else(|| DbError::Validation("Missing compaction owner".into()))?;
    let external = owner["externalId"].as_str().unwrap_or_default();
    let connection = owner["connectionId"].as_str().unwrap_or_default();
    if connection != envelope.connection_id || external.is_empty() {
        return Ok(());
    }
    let id = match &envelope.payload {
        AcpEvent::ToolCall { tool_call_id, .. } | AcpEvent::ToolCallUpdate { tool_call_id, .. } => {
            tool_call_id
        }
        _ => return Ok(()),
    };
    let marker = &meta["contextCompaction"];
    let status = marker["status"].as_str().unwrap_or("in_progress");
    let started = marker["startedAt"]
        .as_i64()
        .or_else(|| owner["observedAt"].as_i64())
        .unwrap_or_else(|| Utc::now().timestamp_millis());
    let run = owner["turnRunId"].as_str().map(str::to_owned);
    // The owner was captured under the event's original state lock. Never
    // substitute a later current run or binding after an async DB wait.
    conn.execute(stmt("INSERT INTO conversation_compaction(conversation_id,external_id,compaction_id,connection_id,turn_run_id,started_at_ms,last_seq,status,meta_json) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(conversation_id,external_id,compaction_id) DO UPDATE SET last_seq=excluded.last_seq,status=excluded.status,meta_json=json_patch(conversation_compaction.meta_json,excluded.meta_json) WHERE conversation_compaction.connection_id=excluded.connection_id AND conversation_compaction.last_seq<excluded.last_seq AND (conversation_compaction.status='in_progress' OR conversation_compaction.status=excluded.status)", vec![cid.into(),external.into(),id.clone().into(),connection.into(),run.into(),started.into(),(envelope.seq as i64).into(),status.into(),json!({"contextCompaction":marker}).to_string().into()])).await?;
    Ok(())
}

pub(crate) async fn interrupt_connection(
    conn: &DatabaseConnection,
    connection: &str,
    cancelled: bool,
    reason: &str,
) -> Result<(), DbError> {
    let status = if cancelled { "cancelled" } else { "failed" };
    conn.execute(stmt("UPDATE conversation_compaction SET status=?,meta_json=json_set(meta_json,'$.contextCompaction.status',?,'$.contextCompaction.phase',?,'$.contextCompaction.error',?) WHERE connection_id=? AND status='in_progress'",vec![status.into(),status.into(),status.into(),reason.into(),connection.into()])).await?;
    Ok(())
}

pub(crate) async fn recover_interrupted(conn: &DatabaseConnection) -> Result<(), DbError> {
    conn.execute(stmt("UPDATE conversation_compaction SET status='failed',meta_json=json_set(meta_json,'$.contextCompaction.status','failed','$.contextCompaction.phase','interrupted','$.contextCompaction.error','CodeG restarted before the compaction outcome was recorded; completion is unknown.') WHERE status='in_progress'",vec![])).await?;
    Ok(())
}

/// Half-open native/business history window. Boundaries come from the already
/// parsed/page-indexed history, never a newest-file or guessed task match.
/// A single indexed query adds only lifecycle metadata inside that window.
pub(crate) async fn merge_history(
    conn: &DatabaseConnection,
    cid: i32,
    external: Option<&str>,
    from: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    turns: &mut Vec<MessageTurn>,
) -> Result<(), DbError> {
    let rows=conn.query_all(stmt("SELECT external_id,compaction_id,started_at_ms,status,meta_json FROM conversation_compaction WHERE conversation_id=? AND (? IS NULL OR external_id=?) AND (? IS NULL OR started_at_ms>=?) AND (? IS NULL OR started_at_ms<?) ORDER BY started_at_ms,compaction_id",vec![cid.into(),external.into(),external.into(),from.map(|t|t.timestamp_millis()).into(),from.map(|t|t.timestamp_millis()).into(),before.map(|t|t.timestamp_millis()).into(),before.map(|t|t.timestamp_millis()).into()])).await?;
    for row in rows {
        let id: String = row.try_get("", "compaction_id")?;
        let external: String = row.try_get("", "external_id")?;
        let status: String = row.try_get("", "status")?;
        let mut meta: Json = serde_json::from_str(&row.try_get::<String>("", "meta_json")?)
            .map_err(|e| DbError::Validation(e.to_string()))?;
        // The ledger's primary key is the original host ACP call id, including
        // rows written before lifecycleId was exposed. Do not infer this link
        // from timestamps or rewrite the native transcript to recover it.
        if let Some(marker) = meta["contextCompaction"].as_object_mut() {
            marker.insert("lifecycleId".into(), json!(id));
        }
        let Some(timestamp) =
            DateTime::<Utc>::from_timestamp_millis(row.try_get("", "started_at_ms")?)
        else {
            continue;
        };
        let event_id = meta["contextCompaction"]["eventId"].as_str().unwrap_or(&id);
        let native_exists = turns.iter().any(|turn| {
            turn.blocks.iter().any(|block| {
                matches!(block, ContentBlock::ToolUse { tool_use_id: Some(call), .. }
                    if call == event_id && call != &id)
            })
        });
        // A native completion can arrive after we captured a provisional
        // standalone ledger record. Keep its richer native summary once only.
        if native_exists {
            turns.retain(|turn| {
                !turn.id.contains("codeg-compaction:")
                    || !turn.blocks.iter().any(|block| {
                        matches!(block, ContentBlock::ToolUse { tool_use_id: Some(call), .. }
                            if call == &id)
                    })
            });
        }
        let mut matched = false;
        for turn in turns.iter_mut() {
            for block in &mut turn.blocks {
                if let ContentBlock::ToolUse {
                    tool_use_id: Some(call),
                    meta: stored,
                    status: call_status,
                    ..
                } = block
                {
                    if call == &id || call == event_id {
                        let stored = stored.get_or_insert_with(|| json!({}));
                        if let (Some(stored), Some(fields)) = (
                            stored.as_object_mut(),
                            meta["contextCompaction"].as_object(),
                        ) {
                            let marker = stored
                                .entry("contextCompaction")
                                .or_insert_with(|| json!({}));
                            if let Some(marker) = marker.as_object_mut() {
                                marker.extend(fields.clone());
                            }
                        }
                        *call_status = Some(status.clone());
                        matched = true;
                    }
                }
            }
        }
        if matched {
            continue;
        }
        let mut blocks = vec![ContentBlock::ToolUse {
            tool_use_id: Some(id.clone()),
            tool_name: "context_compaction".into(),
            input_preview: None,
            status: Some(status.clone()),
            meta: Some(meta),
        }];
        let terminal = status != "in_progress";
        if terminal {
            blocks.push(ContentBlock::ToolResult {
                tool_use_id: Some(id.clone()),
                output_preview: None,
                is_error: status == "failed",
                agent_stats: None,
                images: vec![],
                exit_code: None,
            });
        }
        let record = MessageTurn {
            id: format!("codeg-compaction:{external}:{id}"),
            role: TurnRole::Assistant,
            blocks,
            timestamp,
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
            native_task_id: None,
        };
        let index = turns.partition_point(|t| t.timestamp <= timestamp);
        turns.insert(index, record);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        acp::{compaction::stamp_binding, session_state::SessionState},
        db::test_helpers::*,
        models::AgentType,
    };

    fn frame(
        cid: i32,
        external: &str,
        id: &str,
        seq: u64,
        status: &str,
        started: i64,
        event_id: Option<&str>,
    ) -> EventEnvelope {
        let mut state = SessionState::new("owned".into(), AgentType::Pi, None, "main".into(), None);
        state.conversation_id = Some(cid);
        state.external_id = Some(external.into());
        state.active_turn_run_id = Some("original-run".into());
        let mut marker =
            json!({"version":1,"status":status,"startedAt":started,"trigger":"manual"});
        if status == "failed" {
            marker["error"] = json!("HTTP 403: denied by isolated provider");
        }
        if let Some(event_id) = event_id {
            marker["eventId"] = json!(event_id);
        }
        let mut payload = AcpEvent::ToolCallUpdate {
            tool_call_id: id.into(),
            title: Some("Context compaction".into()),
            status: Some(status.into()),
            content: None,
            raw_input: None,
            raw_output: None,
            raw_output_append: None,
            locations: None,
            meta: Some(json!({"contextCompaction":marker})),
            images: None,
        };
        stamp_binding(&mut payload, &state);
        EventEnvelope {
            connection_id: "owned".into(),
            seq,
            payload,
        }
    }

    async fn fixture() -> (crate::db::AppDatabase, i32) {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/isolated/pi-compaction").await;
        let cid = seed_conversation(&db, folder, AgentType::Pi).await;
        (db, cid)
    }

    fn marker(turn: &MessageTurn) -> &Json {
        match &turn.blocks[0] {
            ContentBlock::ToolUse {
                meta: Some(meta), ..
            } => &meta["contextCompaction"],
            _ => panic!("missing card"),
        }
    }

    #[tokio::test]
    async fn failed_compaction_survives_authoritative_history_reload_without_native_entry() {
        let (db, cid) = fixture().await;
        persist(
            &db.conn,
            &frame(cid, "pi-one", "failed-1", 1, "in_progress", 1000, None),
        )
        .await
        .unwrap();
        persist(
            &db.conn,
            &frame(cid, "pi-one", "failed-1", 2, "failed", 1000, None),
        )
        .await
        .unwrap();
        for _ in 0..3 {
            let mut history = vec![]; // Pi JSONL records no failed compaction.
            merge_history(&db.conn, cid, Some("pi-one"), None, None, &mut history)
                .await
                .unwrap();
            assert_eq!(history.len(), 1);
            assert_eq!(marker(&history[0])["status"], "failed");
            assert_eq!(
                marker(&history[0])["error"],
                "HTTP 403: denied by isolated provider"
            );
            assert_eq!(marker(&history[0])["eventId"], "failed-1");
            assert!(
                history[0].duration_ms.is_none(),
                "compaction time is not task time"
            );
            assert!(crate::models::business_context::history_prose(&history[0]).is_empty());
        }
    }

    #[tokio::test]
    async fn compaction_identity_is_session_scoped_stale_and_duplicate_frames_cannot_reopen_it() {
        let (db, cid) = fixture().await;
        persist(
            &db.conn,
            &frame(cid, "pi-one", "cancel-1", 1, "in_progress", 1000, None),
        )
        .await
        .unwrap();
        persist(
            &db.conn,
            &frame(cid, "pi-one", "cancel-1", 3, "cancelled", 1000, None),
        )
        .await
        .unwrap();
        for seq in [1, 3, 99] {
            persist(
                &db.conn,
                &frame(cid, "pi-one", "cancel-1", seq, "in_progress", 1000, None),
            )
            .await
            .unwrap();
        }
        persist(
            &db.conn,
            &frame(cid, "pi-one", "cancel-1", 100, "completed", 1000, None),
        )
        .await
        .unwrap();
        let mut history = vec![];
        merge_history(&db.conn, cid, Some("pi-two"), None, None, &mut history)
            .await
            .unwrap();
        assert!(history.is_empty(), "never leak another native session");
        merge_history(&db.conn, cid + 1, None, None, None, &mut history)
            .await
            .unwrap();
        assert!(history.is_empty(), "never leak another conversation");
        merge_history(&db.conn, cid, None, None, None, &mut history)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(marker(&history[0])["status"], "cancelled");
    }

    #[tokio::test]
    async fn completed_compaction_enriches_native_record_once_and_preserves_native_summary() {
        let (db, cid) = fixture().await;
        persist(
            &db.conn,
            &frame(cid, "pi-one", "live-id", 1, "in_progress", 1000, None),
        )
        .await
        .unwrap();
        persist(
            &db.conn,
            &frame(
                cid,
                "pi-one",
                "live-id",
                2,
                "completed",
                1000,
                Some("native-entry"),
            ),
        )
        .await
        .unwrap();
        // Simulate an existing ledger row written before the host exposed its
        // stable operation alias; the native entry itself still has another id.
        db.conn.execute(stmt(
            "UPDATE conversation_compaction SET meta_json=json_remove(meta_json,'$.contextCompaction.lifecycleId') WHERE conversation_id=?",
            vec![cid.into()],
        )).await.unwrap();
        let mut native = vec![MessageTurn {
            id: "turn-4".into(),
            role: TurnRole::Assistant,
            timestamp: DateTime::from_timestamp_millis(1200).unwrap(),
            blocks: vec![
                ContentBlock::ToolUse {
                    tool_use_id: Some("native-entry".into()),
                    tool_name: "context_compaction".into(),
                    input_preview: None,
                    status: None,
                    meta: Some(
                        json!({"contextCompaction":{"version":1,"status":"completed","eventId":"native-entry","preTokens":8000},"codeg.compactionSummary":true}),
                    ),
                },
                ContentBlock::ToolResult {
                    tool_use_id: Some("native-entry".into()),
                    output_preview: Some("real retained summary".into()),
                    is_error: false,
                    agent_stats: None,
                    images: vec![],
                    exit_code: None,
                },
            ],
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
            native_task_id: None,
        }];
        for _ in 0..3 {
            merge_history(&db.conn, cid, None, None, None, &mut native)
                .await
                .unwrap();
        }
        assert_eq!(native.len(), 1);
        assert_eq!(marker(&native[0])["eventId"], "native-entry");
        assert_eq!(marker(&native[0])["lifecycleId"], "live-id");
        assert_eq!(marker(&native[0])["preTokens"], 8000);
        assert_eq!(marker(&native[0])["trigger"], "manual");
        assert!(
            matches!(&native[0].blocks[1], ContentBlock::ToolResult {output_preview:Some(summary),..} if summary=="real retained summary")
        );
    }

    #[tokio::test]
    async fn compaction_half_open_pages_include_gap_records_exactly_once() {
        let (db, cid) = fixture().await;
        for (i, started) in [1000, 1999, 2000, 2999].into_iter().enumerate() {
            persist(
                &db.conn,
                &frame(
                    cid,
                    "pi-one",
                    &format!("event-{i}"),
                    1,
                    "failed",
                    started,
                    None,
                ),
            )
            .await
            .unwrap();
        }
        let mut old = vec![];
        let mut newest = vec![];
        let boundary = DateTime::from_timestamp_millis(2000).unwrap();
        merge_history(&db.conn, cid, None, None, Some(boundary), &mut old)
            .await
            .unwrap();
        merge_history(&db.conn, cid, None, Some(boundary), None, &mut newest)
            .await
            .unwrap();
        assert_eq!((old.len(), newest.len()), (2, 2));
        assert!(old.iter().all(|a| newest.iter().all(|b| a.id != b.id)));
    }

    #[tokio::test]
    async fn interrupted_compaction_does_not_spin_forever_or_claim_completion_on_restart() {
        let (db, cid) = fixture().await;
        persist(
            &db.conn,
            &frame(cid, "pi-one", "old-active", 1, "in_progress", 1000, None),
        )
        .await
        .unwrap();
        persist(
            &db.conn,
            &frame(
                cid,
                "pi-one",
                "already-done",
                2,
                "completed",
                2000,
                Some("native-done"),
            ),
        )
        .await
        .unwrap();
        recover_interrupted(&db.conn).await.unwrap();
        recover_interrupted(&db.conn).await.unwrap();
        let mut history = vec![];
        merge_history(&db.conn, cid, None, None, None, &mut history)
            .await
            .unwrap();
        assert_eq!(marker(&history[0])["status"], "failed");
        assert_eq!(marker(&history[0])["phase"], "interrupted");
        assert_eq!(marker(&history[1])["status"], "completed");
    }
}
