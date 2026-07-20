use std::collections::HashSet;

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};

use crate::db::entities::conversation_turn_run::{self, ConversationTurnRunStatus};
use crate::db::entities::{conversation, conversation_deliverable};
use crate::db::error::DbError;
use crate::models::ConversationDeliverable;

#[derive(Debug, Clone)]
pub struct VerifiedDeliverable {
    pub root_path: String,
    pub path: String,
    pub kind: String,
    pub title: String,
    pub description: Option<String>,
    pub role: String,
    pub size_bytes: Option<i64>,
}

fn to_info(model: conversation_deliverable::Model) -> ConversationDeliverable {
    ConversationDeliverable {
        id: model.id,
        conversation_id: model.conversation_id,
        turn_run_id: model.turn_run_id,
        root_path: model.root_path,
        path: model.path,
        kind: model.kind,
        title: model.title,
        description: model.description,
        role: model.role,
        position: model.position,
        source: model.source,
        size_bytes: model.size_bytes,
        verified_at: model.verified_at,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

pub async fn active_turn_run_id(
    conn: &DatabaseConnection,
    conversation_id: i32,
    connection_id: &str,
) -> Result<Option<String>, DbError> {
    Ok(conversation_turn_run::Entity::find()
        .filter(conversation_turn_run::Column::ConversationId.eq(conversation_id))
        .filter(conversation_turn_run::Column::ConnectionId.eq(connection_id))
        .filter(conversation_turn_run::Column::Status.eq(ConversationTurnRunStatus::Running))
        .order_by_desc(conversation_turn_run::Column::StartedAt)
        .one(conn)
        .await?
        .map(|run| run.id))
}

/// Replace the agent-declared final set for a conversation. The conversation,
/// root, and path form the stable identity, so retained outputs keep their ids while
/// omitted agent-declared outputs are removed. Rows from other future sources
/// are intentionally outside this replacement set.
pub async fn upsert_verified(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turn_run_id: Option<String>,
    items: Vec<VerifiedDeliverable>,
) -> Result<Vec<ConversationDeliverable>, DbError> {
    let txn = conn.begin().await?;
    if conversation::Entity::find_by_id(conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(&txn)
        .await?
        .is_none()
    {
        return Err(DbError::NotFound(format!("conversation {conversation_id}")));
    }
    let now = Utc::now();
    let mut saved = Vec::with_capacity(items.len());
    let keep = items
        .iter()
        .map(|item| (item.root_path.clone(), item.path.clone()))
        .collect::<HashSet<_>>();
    let previous = conversation_deliverable::Entity::find()
        .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
        .all(&txn)
        .await?;
    for model in previous {
        if model.source == "agent_declared"
            && !keep.contains(&(model.root_path.clone(), model.path.clone()))
        {
            conversation_deliverable::Entity::delete_by_id(model.id)
                .exec(&txn)
                .await?;
        }
    }

    for (position, item) in items.into_iter().enumerate() {
        let position = i32::try_from(position).unwrap_or(i32::MAX);
        let existing = conversation_deliverable::Entity::find()
            .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
            .filter(conversation_deliverable::Column::RootPath.eq(item.root_path.clone()))
            .filter(conversation_deliverable::Column::Path.eq(item.path.clone()))
            .one(&txn)
            .await?;

        let model = if let Some(existing) = existing {
            let mut active = existing.into_active_model();
            active.turn_run_id = Set(turn_run_id.clone());
            active.kind = Set(item.kind);
            active.title = Set(item.title);
            active.description = Set(item.description);
            active.role = Set(item.role);
            active.position = Set(position);
            active.source = Set("agent_declared".to_string());
            active.size_bytes = Set(item.size_bytes);
            active.verified_at = Set(now);
            active.updated_at = Set(now);
            active.update(&txn).await?
        } else {
            conversation_deliverable::ActiveModel {
                id: Set(uuid::Uuid::new_v4().to_string()),
                conversation_id: Set(conversation_id),
                turn_run_id: Set(turn_run_id.clone()),
                root_path: Set(item.root_path),
                path: Set(item.path),
                kind: Set(item.kind),
                title: Set(item.title),
                description: Set(item.description),
                role: Set(item.role),
                position: Set(position),
                source: Set("agent_declared".to_string()),
                size_bytes: Set(item.size_bytes),
                verified_at: Set(now),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&txn)
            .await?
        };
        saved.push(to_info(model));
    }

    txn.commit().await?;
    Ok(saved)
}

pub async fn list_for_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<ConversationDeliverable>, DbError> {
    Ok(conversation_deliverable::Entity::find()
        .filter(conversation_deliverable::Column::ConversationId.eq(conversation_id))
        .order_by_asc(conversation_deliverable::Column::Position)
        .all(conn)
        .await?
        .into_iter()
        .map(to_info)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AgentType;

    #[tokio::test]
    async fn repeated_path_refreshes_one_conversation_deliverable() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let folder_id = crate::db::test_helpers::seed_folder(&db, "/tmp/deliverables").await;
        let conversation_id =
            crate::db::test_helpers::seed_conversation(&db, folder_id, AgentType::Codex).await;

        let first = VerifiedDeliverable {
            root_path: "/tmp/deliverables".into(),
            path: "report.pdf".into(),
            kind: "file".into(),
            title: "Report".into(),
            description: None,
            role: "primary".into(),
            size_bytes: Some(12),
        };
        let supporting = VerifiedDeliverable {
            root_path: "/tmp/deliverables".into(),
            path: "appendix.csv".into(),
            kind: "file".into(),
            title: "Appendix".into(),
            description: None,
            role: "supporting".into(),
            size_bytes: Some(8),
        };
        let first_saved = upsert_verified(&db.conn, conversation_id, None, vec![first, supporting])
            .await
            .expect("first declaration");
        assert_eq!(first_saved.len(), 2);
        let retained_id = first_saved[0].id.clone();

        let refreshed = VerifiedDeliverable {
            root_path: "/tmp/deliverables".into(),
            path: "report.pdf".into(),
            kind: "file".into(),
            title: "Final report".into(),
            description: Some("Ready for delivery".into()),
            role: "primary".into(),
            size_bytes: Some(24),
        };
        upsert_verified(&db.conn, conversation_id, None, vec![refreshed])
            .await
            .expect("refresh declaration");

        let listed = list_for_conversation(&db.conn, conversation_id)
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, retained_id);
        assert_eq!(listed[0].title, "Final report");
        assert_eq!(listed[0].size_bytes, Some(24));
    }
}
