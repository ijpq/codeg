use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set, TransactionError, TransactionTrait,
};
use serde::{Deserialize, Serialize};

use crate::db::entities::{
    conversation, conversation_branch, conversation_branch_merge, conversation_deliverable,
    conversation_turn_deliverable, conversation_turn_run,
};
use crate::db::error::DbError;
use crate::db::service::conversation_service;
use crate::models::conversation::DbConversationSummary;
use crate::models::message::{ContentBlock, MessageTurn, TurnRole};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationBranchInfo {
    pub branch_conversation_id: i32,
    pub source_conversation_id: i32,
    pub source_title: Option<String>,
    pub source_available: bool,
    pub fork_message_id: Option<String>,
    pub fork_mode: String,
    pub created_at: chrono::DateTime<Utc>,
    pub last_merged_at: Option<chrono::DateTime<Utc>>,
    pub merge_target_conversation_id: Option<i32>,
}

impl From<conversation_branch::Model> for ConversationBranchInfo {
    fn from(value: conversation_branch::Model) -> Self {
        Self {
            branch_conversation_id: value.branch_conversation_id,
            source_conversation_id: value.source_conversation_id,
            source_title: value.source_title,
            source_available: true,
            fork_message_id: value.fork_message_id,
            fork_mode: value.fork_mode,
            created_at: value.created_at,
            last_merged_at: value.last_merged_at,
            merge_target_conversation_id: value.merge_target_conversation_id,
        }
    }
}

pub async fn create_branch_row(
    conn: &DatabaseConnection,
    source: &DbConversationSummary,
    external_id: Option<String>,
    fork_message_id: Option<String>,
    fork_mode: &str,
    snapshot_context: Option<String>,
) -> Result<(conversation::Model, ConversationBranchInfo), DbError> {
    let title = Some(format!(
        "{} · 分支",
        source.title.as_deref().unwrap_or("未命名会话")
    ));
    let created = if source.kind == conversation::ConversationKind::Chat {
        conversation_service::create_chat(
            conn,
            source.folder_id,
            source.agent_type,
            title,
            source.git_branch.clone(),
        )
        .await?
    } else {
        conversation_service::create(
            conn,
            source.folder_id,
            source.agent_type,
            title,
            source.git_branch.clone(),
        )
        .await?
    };

    let mut active = created.into_active_model();
    active.title_locked = Set(true);
    active.model = Set(source.model.clone());
    active.external_id = Set(external_id);
    active.origin_cwd = Set(source.origin_cwd.clone());
    let created = active.update(conn).await?;
    let now = Utc::now();
    let relation = conversation_branch::ActiveModel {
        branch_conversation_id: Set(created.id),
        source_conversation_id: Set(source.id),
        source_title: Set(source.title.clone()),
        fork_message_id: Set(fork_message_id),
        fork_mode: Set(fork_mode.to_string()),
        snapshot_context: Set(snapshot_context),
        snapshot_consumed_at: Set(None),
        created_at: Set(now),
        last_merged_at: Set(None),
        last_merge_key: Set(None),
        merge_target_conversation_id: Set(None),
    }
    .insert(conn)
    .await?;
    Ok((created, relation.into()))
}

pub async fn get_info(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Option<ConversationBranchInfo>, DbError> {
    let Some(row) = conversation_branch::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(None);
    };
    let source = conversation::Entity::find_by_id(row.source_conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?;
    let mut info: ConversationBranchInfo = row.into();
    info.source_available = source.is_some();
    if let Some(source) = source {
        info.source_title = source.title;
    }
    Ok(Some(info))
}

pub async fn pending_snapshot(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Option<String>, DbError> {
    Ok(conversation_branch::Entity::find_by_id(conversation_id)
        .filter(conversation_branch::Column::SnapshotConsumedAt.is_null())
        .one(conn)
        .await?
        .and_then(|row| row.snapshot_context)
        .filter(|text| !text.trim().is_empty()))
}

pub async fn mark_snapshot_consumed(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<(), DbError> {
    if let Some(row) = conversation_branch::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    {
        let mut active = row.into_active_model();
        active.snapshot_consumed_at = Set(Some(Utc::now()));
        active.update(conn).await?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeBranchResult {
    pub merge_id: String,
    pub target_conversation_id: i32,
    pub copied_deliverable_count: usize,
    pub deduplicated: bool,
}

pub async fn merge_branch(
    conn: &DatabaseConnection,
    branch_conversation_id: i32,
    request_id: String,
    summary: String,
    deliverable_ids: Vec<String>,
) -> Result<MergeBranchResult, DbError> {
    let request_id = request_id.trim().to_string();
    if request_id.is_empty() || summary.trim().is_empty() {
        return Err(DbError::Validation(
            "request_id and merge summary are required".into(),
        ));
    }
    if let Some(existing) = conversation_branch_merge::Entity::find_by_id(&request_id)
        .one(conn)
        .await?
    {
        let copied = serde_json::from_str::<Vec<String>>(&existing.deliverable_ids_json)
            .unwrap_or_default()
            .len();
        return Ok(MergeBranchResult {
            merge_id: existing.id,
            target_conversation_id: existing.target_conversation_id,
            copied_deliverable_count: copied,
            deduplicated: true,
        });
    }
    let relation = conversation_branch::Entity::find_by_id(branch_conversation_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound("conversation branch".into()))?;
    // Deletion is soft and relations intentionally do not cascade. Reject a
    // merge when the source is gone; deleting either side never deletes the
    // other side or the audit row.
    conversation::Entity::find_by_id(relation.source_conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound("source conversation".into()))?;

    let selected = if deliverable_ids.is_empty() {
        Vec::new()
    } else {
        conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(branch_conversation_id))
            .filter(conversation_deliverable::Column::Id.is_in(deliverable_ids.clone()))
            .filter(conversation_deliverable::Column::IsValid.eq(true))
            .filter(conversation_deliverable::Column::IsHidden.eq(false))
            .all(conn)
            .await?
    };
    let copied_count = selected.len();
    let copied_source_ids: Vec<String> = selected.iter().map(|d| d.id.clone()).collect();
    let now = Utc::now();
    let target_id = relation.source_conversation_id;
    let run_id = format!("branch-merge-{request_id}");
    let client_message_id = run_id.clone();
    let root_path = selected
        .first()
        .map(|item| item.root_path.clone())
        .unwrap_or_default();
    let summary_for_tx = summary.trim().to_string();
    let request_for_tx = request_id.clone();
    let transaction_result = conn
        .transaction::<_, (), sea_orm::DbErr>(|txn| {
            Box::pin(async move {
                conversation_branch_merge::ActiveModel {
                    id: Set(request_for_tx.clone()),
                    branch_conversation_id: Set(branch_conversation_id),
                    source_conversation_id: Set(target_id),
                    target_conversation_id: Set(target_id),
                    summary: Set(summary_for_tx),
                    deliverable_ids_json: Set(
                        serde_json::to_string(&copied_source_ids).unwrap_or_else(|_| "[]".into())
                    ),
                    created_at: Set(now),
                    context_consumed_at: Set(None),
                }
                .insert(txn)
                .await?;
                conversation_turn_run::ActiveModel {
                    id: Set(run_id.clone()),
                    conversation_id: Set(target_id),
                    connection_id: Set(format!("branch-merge:{branch_conversation_id}")),
                    client_message_id: Set(Some(client_message_id)),
                    prompt_accepted_at: Set(Some(now)),
                    prompt_fingerprint: Set(None),
                    folder_id: Set(None),
                    root_path: Set(root_path),
                    status: Set(conversation_turn_run::ConversationTurnRunStatus::Completed),
                    capture_incomplete: Set(false),
                    stop_reason: Set(Some("branch_merge".into())),
                    started_at: Set(now),
                    completed_at: Set(Some(now)),
                    deliverables_declared_at: Set((!selected.is_empty()).then_some(now)),
                    input_paths_json: Set("[]".into()),
                    declaration_status: Set(if selected.is_empty() {
                        "success_empty".into()
                    } else {
                        "success".into()
                    }),
                    declaration_attempted_at: Set(Some(now)),
                    declaration_error: Set(None),
                    expectation_json: Set("{}".into()),
                    settlement_status: Set("settled".into()),
                    settled_at: Set(Some(now)),
                    missing_expected_paths_json: Set("[]".into()),
                }
                .insert(txn)
                .await?;
                for (position, source) in selected.into_iter().enumerate() {
                    let copied_id = uuid::Uuid::new_v4().to_string();
                    conversation_deliverable::ActiveModel {
                        id: Set(copied_id.clone()),
                        conversation_id: Set(target_id),
                        turn_run_id: Set(Some(run_id.clone())),
                        root_path: Set(source.root_path),
                        path: Set(source.path),
                        kind: Set(source.kind),
                        title: Set(source.title.clone()),
                        description: Set(source.description),
                        role: Set(source.role.clone()),
                        category: Set(source.category.clone()),
                        change_kind: Set(source.change_kind.clone()),
                        position: Set(position as i32),
                        source: Set("branch_merge".into()),
                        file_name: Set(source.file_name),
                        extension: Set(source.extension),
                        size_bytes: Set(source.size_bytes),
                        modified_at: Set(source.modified_at),
                        is_valid: Set(source.is_valid),
                        invalid_reason: Set(source.invalid_reason),
                        is_hidden: Set(false),
                        verified_at: Set(source.verified_at),
                        last_checked_at: Set(source.last_checked_at),
                        created_at: Set(now),
                        updated_at: Set(now),
                    }
                    .insert(txn)
                    .await?;
                    conversation_turn_deliverable::ActiveModel {
                        id: Set(uuid::Uuid::new_v4().to_string()),
                        conversation_id: Set(target_id),
                        turn_run_id: Set(run_id.clone()),
                        deliverable_id: Set(copied_id),
                        source: Set("branch_merge".into()),
                        title: Set(source.title),
                        description: Set(None),
                        role: Set(source.role),
                        category: Set(source.category),
                        change_kind: Set(source.change_kind),
                        position: Set(position as i32),
                        created_at: Set(now),
                        updated_at: Set(now),
                    }
                    .insert(txn)
                    .await?;
                }
                let mut active = relation.into_active_model();
                active.last_merged_at = Set(Some(now));
                active.last_merge_key = Set(Some(request_for_tx));
                active.merge_target_conversation_id = Set(Some(target_id));
                active.update(txn).await?;
                Ok(())
            })
        })
        .await;
    if let Err(error) = transaction_result {
        // Two identical merge clicks can both pass the optimistic pre-read.
        // The primary key is the final arbiter; the loser re-reads the winner
        // and reports a deduplicated success instead of surfacing SQLITE UNIQUE.
        if let Some(existing) = conversation_branch_merge::Entity::find_by_id(&request_id)
            .one(conn)
            .await?
        {
            let copied = serde_json::from_str::<Vec<String>>(&existing.deliverable_ids_json)
                .unwrap_or_default()
                .len();
            return Ok(MergeBranchResult {
                merge_id: existing.id,
                target_conversation_id: existing.target_conversation_id,
                copied_deliverable_count: copied,
                deduplicated: true,
            });
        }
        return Err(match error {
            TransactionError::Connection(error) | TransactionError::Transaction(error) => {
                DbError::Database(error)
            }
        });
    }

    Ok(MergeBranchResult {
        merge_id: request_id,
        target_conversation_id: target_id,
        copied_deliverable_count: copied_count,
        deduplicated: false,
    })
}

pub async fn merge_turns_for_target(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<MessageTurn>, DbError> {
    let rows = conversation_branch_merge::Entity::find()
        .filter(conversation_branch_merge::Column::TargetConversationId.eq(conversation_id))
        .order_by_asc(conversation_branch_merge::Column::CreatedAt)
        .all(conn)
        .await?;
    let mut turns = Vec::with_capacity(rows.len() * 2);
    for row in rows {
        let user_id = format!("branch-merge-{}", row.id);
        turns.push(MessageTurn {
            id: user_id,
            role: TurnRole::User,
            blocks: vec![ContentBlock::Text {
                text: format!("合并分支 #{} 的成果", row.branch_conversation_id),
            }],
            timestamp: row.created_at,
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: Some(row.created_at),
        });
        turns.push(MessageTurn {
            id: format!("branch-merge-{}-result", row.id),
            role: TurnRole::Assistant,
            blocks: vec![ContentBlock::Text { text: row.summary }],
            timestamp: row.created_at,
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: Some(row.created_at),
        });
    }
    Ok(turns)
}

pub async fn pending_merge_context(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<(String, String)>, DbError> {
    let rows = conversation_branch_merge::Entity::find()
        .filter(conversation_branch_merge::Column::TargetConversationId.eq(conversation_id))
        .filter(conversation_branch_merge::Column::ContextConsumedAt.is_null())
        .order_by_asc(conversation_branch_merge::Column::CreatedAt)
        .all(conn)
        .await?;
    let mut contexts = Vec::with_capacity(rows.len());
    for row in rows {
        let run_id = format!("branch-merge-{}", row.id);
        let files = conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
            .filter(conversation_deliverable::Column::TurnRunId.eq(run_id))
            .filter(conversation_deliverable::Column::IsValid.eq(true))
            .filter(conversation_deliverable::Column::IsHidden.eq(false))
            .order_by_asc(conversation_deliverable::Column::Position)
            .all(conn)
            .await?;
        let mut text = format!(
            "Merged from user branch #{} at {}:\n{}",
            row.branch_conversation_id,
            row.created_at.to_rfc3339(),
            row.summary
        );
        if !files.is_empty() {
            text.push_str("\nSelected deliverables:");
            for file in files {
                text.push_str(&format!("\n- {}", file.path));
            }
        }
        contexts.push((row.id, text));
    }
    Ok(contexts)
}

pub async fn mark_merge_context_consumed(
    conn: &DatabaseConnection,
    conversation_id: i32,
    merge_ids: &[String],
) -> Result<(), DbError> {
    if merge_ids.is_empty() {
        return Ok(());
    }
    conversation_branch_merge::Entity::update_many()
        .col_expr(
            conversation_branch_merge::Column::ContextConsumedAt,
            sea_orm::sea_query::Expr::value(Utc::now()),
        )
        .filter(conversation_branch_merge::Column::TargetConversationId.eq(conversation_id))
        .filter(conversation_branch_merge::Column::Id.is_in(merge_ids.to_vec()))
        .filter(conversation_branch_merge::Column::ContextConsumedAt.is_null())
        .exec(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::models::AgentType;

    #[tokio::test]
    async fn snapshot_branch_persists_relation_without_delegate_parentage() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-branch-test").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let source = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();

        let (branch, info) = create_branch_row(
            &db.conn,
            &source,
            None,
            Some("turn-3".into()),
            "snapshot",
            Some("User: hello".into()),
        )
        .await
        .unwrap();

        assert_ne!(branch.id, source_id);
        assert_eq!(branch.folder_id, folder_id);
        assert_eq!(branch.parent_id, None);
        assert_eq!(branch.kind, conversation::ConversationKind::Regular);
        assert_eq!(info.source_conversation_id, source_id);
        assert!(info.source_available);
        assert_eq!(info.fork_message_id.as_deref(), Some("turn-3"));
        assert_eq!(
            pending_snapshot(&db.conn, branch.id)
                .await
                .unwrap()
                .as_deref(),
            Some("User: hello")
        );
        mark_snapshot_consumed(&db.conn, branch.id).await.unwrap();
        assert_eq!(pending_snapshot(&db.conn, branch.id).await.unwrap(), None);

        conversation_service::soft_delete(&db.conn, source_id)
            .await
            .unwrap();
        let info = get_info(&db.conn, branch.id).await.unwrap().unwrap();
        assert!(!info.source_available);
        assert!(conversation_service::get_by_id(&db.conn, branch.id)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn merge_is_append_only_and_idempotent() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-branch-merge-test").await;
        let source_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let source = conversation_service::get_by_id(&db.conn, source_id)
            .await
            .unwrap();
        let (branch, _) = create_branch_row(
            &db.conn,
            &source,
            Some("fork-session".into()),
            None,
            "native",
            None,
        )
        .await
        .unwrap();

        let now = Utc::now();
        for (id, path, valid, hidden) in [
            ("selected", "final.pdf", true, false),
            ("not-selected", "notes.txt", true, false),
            ("invalid", "missing.pdf", false, false),
            ("hidden", "internal.log", true, true),
        ] {
            conversation_deliverable::ActiveModel {
                id: Set(id.into()),
                conversation_id: Set(branch.id),
                turn_run_id: Set(None),
                root_path: Set("/tmp/codeg-branch-merge-test".into()),
                path: Set(path.into()),
                kind: Set("file".into()),
                title: Set(path.into()),
                description: Set(None),
                role: Set("primary".into()),
                category: Set("document".into()),
                change_kind: Set("created".into()),
                position: Set(0),
                source: Set("declared".into()),
                file_name: Set(path.into()),
                extension: Set(path.rsplit_once('.').map(|(_, ext)| ext.into())),
                size_bytes: Set(Some(42)),
                modified_at: Set(Some(now)),
                is_valid: Set(valid),
                invalid_reason: Set((!valid).then(|| "missing".into())),
                is_hidden: Set(hidden),
                verified_at: Set(now),
                last_checked_at: Set(Some(now)),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&db.conn)
            .await
            .unwrap();
        }

        let first = merge_branch(
            &db.conn,
            branch.id,
            "merge-once".into(),
            "Branch conclusion".into(),
            vec!["selected".into(), "invalid".into(), "hidden".into()],
        )
        .await
        .unwrap();
        let retry = merge_branch(
            &db.conn,
            branch.id,
            "merge-once".into(),
            "ignored duplicate".into(),
            vec!["selected".into(), "not-selected".into()],
        )
        .await
        .unwrap();
        assert!(!first.deduplicated);
        assert!(retry.deduplicated);
        assert_eq!(first.copied_deliverable_count, 1);
        assert_eq!(retry.copied_deliverable_count, 1);
        let copied = conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(source_id))
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(copied.len(), 1);
        assert_eq!(copied[0].path, "final.pdf");
        assert_eq!(copied[0].source, "branch_merge");
        let pending = pending_merge_context(&db.conn, source_id).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].1.contains("Branch conclusion"));
        assert!(pending[0].1.contains("final.pdf"));
        assert!(!pending[0].1.contains("notes.txt"));
        mark_merge_context_consumed(&db.conn, source_id, &[pending[0].0.clone()])
            .await
            .unwrap();
        assert!(pending_merge_context(&db.conn, source_id)
            .await
            .unwrap()
            .is_empty());
        let turns = merge_turns_for_target(&db.conn, source_id).await.unwrap();
        assert_eq!(turns.len(), 2);
        assert!(matches!(turns[0].role, TurnRole::User));
        assert!(matches!(turns[1].role, TurnRole::Assistant));
        let ContentBlock::Text { text } = &turns[1].blocks[0] else {
            panic!("expected merge summary text")
        };
        assert_eq!(text, "Branch conclusion");

        // Both conversations remain independent live rows.
        assert!(conversation_service::get_by_id(&db.conn, source_id)
            .await
            .is_ok());
        assert!(conversation_service::get_by_id(&db.conn, branch.id)
            .await
            .is_ok());

        conversation_service::soft_delete(&db.conn, branch.id)
            .await
            .unwrap();
        assert!(conversation_service::get_by_id(&db.conn, source_id)
            .await
            .is_ok());
    }
}
