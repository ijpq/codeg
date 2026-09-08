use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::Statement;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationBranch::Table)
                    .add_column(
                        ColumnDef::new(ConversationBranch::OperationId)
                            .string()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        let backend = manager.get_database_backend();
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                "UPDATE conversation_branch SET operation_id = creation_request_id \
                 WHERE operation_id IS NULL AND creation_request_id IS NOT NULL"
                    .to_owned(),
            ))
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_branch_operation_id")
                    .table(ConversationBranch::Table)
                    .col(ConversationBranch::OperationId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Repair the exact legacy half-branch shape seen in production. An
        // ephemeral session/new connection accepting prompts is not a verified
        // durable branch session. Preserve its snapshot and make first use
        // retryable instead of presenting it as ready.
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                "UPDATE conversation_branch SET lifecycle_state = 'provisional', \
                   lifecycle_error = COALESCE(lifecycle_error, \
                     'The provisional branch session was not verified; it will be initialized on first use.'), \
                   lifecycle_updated_at = CURRENT_TIMESTAMP, last_connection_id = NULL \
                 WHERE fork_mode = 'snapshot' AND snapshot_consumed_at IS NULL \
                   AND branch_session_id IS NULL AND session_verified_at IS NULL \
                   AND lifecycle_state = 'prompt_ready'"
                    .to_owned(),
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_conversation_branch_operation_id")
                    .table(ConversationBranch::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationBranch::Table)
                    .drop_column(ConversationBranch::OperationId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ConversationBranch {
    Table,
    OperationId,
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
    async fn repairs_post_upgrade_prompt_ready_snapshot_and_backfills_operation() {
        let conn = Database::connect("sqlite::memory:").await.unwrap();
        let migrations = <Migrator as MigratorTrait>::migrations();
        let this = migrations
            .iter()
            .position(|migration| migration.name().contains("branch_operation_id"))
            .unwrap();
        Migrator::up(&conn, Some(this as u32)).await.unwrap();
        conn.execute(sql("INSERT INTO folder (id,name,path,last_opened_at,created_at,updated_at,is_open,sort_order,color,kind) VALUES (1,'f','/tmp/f',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,1,0,'inherit','regular')")).await.unwrap();
        for id in [1, 2] {
            conn.execute(sql(&format!("INSERT INTO conversation (id,folder_id,title,title_locked,agent_type,status,kind,message_count,created_at,updated_at) VALUES ({id},1,'b{id}',1,'codex','pending_review','regular',0,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)"))).await.unwrap();
        }
        conn.execute(sql("INSERT INTO conversation_branch (branch_conversation_id,creation_request_id,source_conversation_id,fork_mode,inheritance_mode,inherited_message_count,inherited_context_chars,inherited_estimated_tokens,inheritance_compressed,inheritance_truncated,snapshot_version,snapshot_context,lifecycle_state,created_at) VALUES (2,'operation-1',1,'snapshot','full_replay',1,10,3,0,0,2,'context','prompt_ready',CURRENT_TIMESTAMP)")).await.unwrap();

        Migrator::up(&conn, None).await.unwrap();
        let repaired = conn.query_one(sql("SELECT operation_id,lifecycle_state,branch_session_id,session_verified_at FROM conversation_branch WHERE branch_conversation_id=2")).await.unwrap().unwrap();
        assert_eq!(
            repaired.try_get::<String>("", "operation_id").unwrap(),
            "operation-1"
        );
        assert_eq!(
            repaired.try_get::<String>("", "lifecycle_state").unwrap(),
            "provisional"
        );
        assert!(repaired
            .try_get::<Option<String>>("", "branch_session_id")
            .unwrap()
            .is_none());
        assert!(repaired
            .try_get::<Option<String>>("", "session_verified_at")
            .unwrap()
            .is_none());
    }
}
