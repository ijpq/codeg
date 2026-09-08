use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::Statement;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let columns = [
            ColumnDef::new(ConversationBranch::LifecycleState)
                .string()
                .not_null()
                .default("ready")
                .to_owned(),
            ColumnDef::new(ConversationBranch::LifecycleError)
                .text()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::LifecycleUpdatedAt)
                .timestamp_with_time_zone()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::SessionVerifiedAt)
                .timestamp_with_time_zone()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::FirstPromptClientMessageId)
                .string()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::FirstPromptQueuedAt)
                .timestamp_with_time_zone()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::FirstPromptAcceptedAt)
                .timestamp_with_time_zone()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::InitializationRetryCount)
                .integer()
                .not_null()
                .default(0)
                .to_owned(),
            ColumnDef::new(ConversationBranch::LastConnectionId)
                .string()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::SnapshotDigest)
                .string()
                .null()
                .to_owned(),
        ];
        for column in columns {
            manager
                .alter_table(
                    Table::alter()
                        .table(ConversationBranch::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }

        // A snapshot branch with no accepted message/turn has no durable Codex
        // rollout, even if an older build wrote session/new's in-memory id into
        // both rows. Repair only that provably-empty shape. User history and the
        // unconsumed snapshot stay untouched and can initialize on first send.
        let backend = manager.get_database_backend();
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                "UPDATE conversation_branch SET \
                   lifecycle_state = 'provisional', \
                   lifecycle_updated_at = CURRENT_TIMESTAMP, \
                   branch_session_id = NULL, \
                   lifecycle_error = CASE \
                     WHEN branch_session_id IS NOT NULL THEN 'Legacy empty snapshot session was not durable and will be recreated.' \
                     ELSE lifecycle_error END \
                 WHERE fork_mode = 'snapshot' \
                   AND snapshot_consumed_at IS NULL \
                   AND snapshot_context IS NOT NULL \
                   AND TRIM(snapshot_context) <> '' \
                   AND EXISTS (SELECT 1 FROM conversation c \
                               WHERE c.id = conversation_branch.branch_conversation_id \
                                 AND c.message_count = 0) \
                   AND NOT EXISTS (SELECT 1 FROM conversation_turn_run r \
                                   WHERE r.conversation_id = conversation_branch.branch_conversation_id)"
                    .to_owned(),
            ))
            .await?;
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                "UPDATE conversation SET external_id = NULL, status = 'pending_review' \
                 WHERE id IN (SELECT branch_conversation_id FROM conversation_branch \
                              WHERE lifecycle_state = 'provisional')"
                    .to_owned(),
            ))
            .await?;
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                "UPDATE conversation_branch SET \
                   lifecycle_state = 'ready', \
                   lifecycle_updated_at = COALESCE(snapshot_consumed_at, created_at), \
                   session_verified_at = snapshot_consumed_at, \
                   first_prompt_accepted_at = snapshot_consumed_at \
                 WHERE lifecycle_state <> 'provisional'"
                    .to_owned(),
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            ConversationBranch::SnapshotDigest,
            ConversationBranch::LastConnectionId,
            ConversationBranch::InitializationRetryCount,
            ConversationBranch::FirstPromptAcceptedAt,
            ConversationBranch::FirstPromptQueuedAt,
            ConversationBranch::FirstPromptClientMessageId,
            ConversationBranch::SessionVerifiedAt,
            ConversationBranch::LifecycleUpdatedAt,
            ConversationBranch::LifecycleError,
            ConversationBranch::LifecycleState,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(ConversationBranch::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum ConversationBranch {
    Table,
    LifecycleState,
    LifecycleError,
    LifecycleUpdatedAt,
    SessionVerifiedAt,
    FirstPromptClientMessageId,
    FirstPromptQueuedAt,
    FirstPromptAcceptedAt,
    InitializationRetryCount,
    LastConnectionId,
    SnapshotDigest,
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
    use sea_orm_migration::MigratorTrait;

    use crate::db::migration::Migrator;

    fn sql(value: &str) -> Statement {
        Statement::from_string(DbBackend::Sqlite, value.to_owned())
    }

    #[tokio::test]
    async fn repairs_only_empty_unconsumed_snapshot_branches() {
        let conn = Database::connect("sqlite::memory:").await.unwrap();
        let migrations = <Migrator as MigratorTrait>::migrations();
        let this = migrations
            .iter()
            .position(|migration| migration.name().contains("branch_lifecycle"))
            .unwrap();
        Migrator::up(&conn, Some(this as u32)).await.unwrap();
        conn.execute(sql("INSERT INTO folder (id,name,path,last_opened_at,created_at,updated_at,is_open,sort_order,color,kind) VALUES (1,'f','/tmp/f',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,1,0,'inherit','regular')")).await.unwrap();
        for (id, count, external) in [(1, 0, "fake-empty"), (2, 1, "real-used")] {
            conn.execute(sql(&format!("INSERT INTO conversation (id,folder_id,title,title_locked,agent_type,status,kind,external_id,message_count,created_at,updated_at) VALUES ({id},1,'b',1,'codex','cancelled','regular','{external}',{count},CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)"))).await.unwrap();
            conn.execute(sql(&format!("INSERT INTO conversation_branch (branch_conversation_id,source_conversation_id,fork_mode,branch_session_id,inheritance_mode,inherited_message_count,inherited_context_chars,inherited_estimated_tokens,inheritance_compressed,inheritance_truncated,snapshot_version,snapshot_context,created_at) VALUES ({id},99,'snapshot','{external}','structured_snapshot',1,1,1,0,0,1,'context',CURRENT_TIMESTAMP)"))).await.unwrap();
        }
        Migrator::up(&conn, None).await.unwrap();

        let empty = conn.query_one(sql("SELECT b.lifecycle_state,b.branch_session_id,c.external_id,c.status FROM conversation_branch b JOIN conversation c ON c.id=b.branch_conversation_id WHERE b.branch_conversation_id=1")).await.unwrap().unwrap();
        assert_eq!(
            empty.try_get::<String>("", "lifecycle_state").unwrap(),
            "provisional"
        );
        assert_eq!(
            empty
                .try_get::<Option<String>>("", "branch_session_id")
                .unwrap(),
            None
        );
        assert_eq!(
            empty.try_get::<Option<String>>("", "external_id").unwrap(),
            None
        );
        assert_eq!(
            empty.try_get::<String>("", "status").unwrap(),
            "pending_review"
        );

        let used = conn.query_one(sql("SELECT b.lifecycle_state,b.branch_session_id,c.external_id FROM conversation_branch b JOIN conversation c ON c.id=b.branch_conversation_id WHERE b.branch_conversation_id=2")).await.unwrap().unwrap();
        assert_eq!(
            used.try_get::<String>("", "lifecycle_state").unwrap(),
            "ready"
        );
        assert_eq!(
            used.try_get::<String>("", "branch_session_id").unwrap(),
            "real-used"
        );
        assert_eq!(
            used.try_get::<String>("", "external_id").unwrap(),
            "real-used"
        );
    }
}
