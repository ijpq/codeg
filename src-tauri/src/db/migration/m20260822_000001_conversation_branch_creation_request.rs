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
                        ColumnDef::new(ConversationBranch::CreationRequestId)
                            .string()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_branch_creation_request")
                    .table(ConversationBranch::Table)
                    .col(ConversationBranch::CreationRequestId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        let backend = manager.get_database_backend();
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                "UPDATE conversation_branch SET lifecycle_state = 'provisional', \
                   lifecycle_updated_at = CURRENT_TIMESTAMP, session_verified_at = NULL, \
                   branch_session_id = NULL, last_connection_id = NULL \
                 WHERE fork_mode = 'snapshot' AND snapshot_consumed_at IS NULL \
                   AND session_verified_at IS NULL AND branch_session_id IS NULL \
                   AND lifecycle_state = 'prompt_ready'"
                    .to_owned(),
            ))
            .await?;
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                "UPDATE conversation_branch SET lifecycle_state = 'failed', \
                   lifecycle_error = COALESCE(lifecycle_error, \
                     'Legacy native branch has no verified branch session; return to the source and recreate it.'), \
                   lifecycle_updated_at = CURRENT_TIMESTAMP, last_connection_id = NULL \
                 WHERE fork_mode = 'native' AND branch_session_id IS NULL \
                   AND session_verified_at IS NULL \
                   AND lifecycle_state IN ('prompt_ready', 'ready')"
                    .to_owned(),
            ))
            .await?;
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                "UPDATE conversation_branch SET lifecycle_state = 'merged', \
                   lifecycle_updated_at = COALESCE(last_merged_at, CURRENT_TIMESTAMP) \
                 WHERE last_merged_at IS NOT NULL"
                    .to_owned(),
            ))
            .await?;
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                "UPDATE conversation SET status = 'completed', pinned_at = NULL \
                 WHERE id IN (SELECT branch_conversation_id FROM conversation_branch \
                              WHERE lifecycle_state = 'merged')"
                    .to_owned(),
            ))
            .await?;
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                "DELETE FROM opened_tab WHERE conversation_id IN (SELECT branch_conversation_id \
                 FROM conversation_branch WHERE lifecycle_state = 'merged')"
                    .to_owned(),
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_conversation_branch_creation_request")
                    .table(ConversationBranch::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationBranch::Table)
                    .drop_column(ConversationBranch::CreationRequestId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ConversationBranch {
    Table,
    CreationRequestId,
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
    async fn upgrades_half_ready_and_merged_branches_without_deleting_history() {
        let conn = Database::connect("sqlite::memory:").await.unwrap();
        let migrations = <Migrator as MigratorTrait>::migrations();
        let this = migrations
            .iter()
            .position(|migration| migration.name().contains("branch_creation_request"))
            .unwrap();
        Migrator::up(&conn, Some(this as u32)).await.unwrap();
        conn.execute(sql("INSERT INTO folder (id,name,path,last_opened_at,created_at,updated_at,is_open,sort_order,color,kind) VALUES (1,'f','/tmp/f',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,1,0,'inherit','regular')")).await.unwrap();
        for (id, status) in [
            (1, "pending_review"),
            (2, "in_progress"),
            (3, "in_progress"),
            (4, "pending_review"),
        ] {
            conn.execute(sql(&format!("INSERT INTO conversation (id,folder_id,title,title_locked,agent_type,status,kind,message_count,created_at,updated_at) VALUES ({id},1,'b{id}',1,'codex','{status}','regular',0,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)"))).await.unwrap();
        }
        conn.execute(sql("INSERT INTO conversation_branch (branch_conversation_id,source_conversation_id,fork_mode,inheritance_mode,inherited_message_count,inherited_context_chars,inherited_estimated_tokens,inheritance_compressed,inheritance_truncated,snapshot_version,snapshot_context,lifecycle_state,created_at) VALUES (2,1,'snapshot','full_replay',1,10,3,0,0,2,'context','prompt_ready',CURRENT_TIMESTAMP)")).await.unwrap();
        conn.execute(sql("INSERT INTO conversation_branch (branch_conversation_id,source_conversation_id,fork_mode,inheritance_mode,inherited_message_count,inherited_context_chars,inherited_estimated_tokens,inheritance_compressed,inheritance_truncated,snapshot_version,lifecycle_state,last_merged_at,created_at) VALUES (3,1,'native','native_fork',1,0,0,0,0,2,'ready',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)")).await.unwrap();
        conn.execute(sql("INSERT INTO conversation_branch (branch_conversation_id,source_conversation_id,fork_mode,inheritance_mode,inherited_message_count,inherited_context_chars,inherited_estimated_tokens,inheritance_compressed,inheritance_truncated,snapshot_version,lifecycle_state,created_at) VALUES (4,1,'native','native_fork',1,0,0,0,0,2,'prompt_ready',CURRENT_TIMESTAMP)")).await.unwrap();
        conn.execute(sql("INSERT INTO opened_tab (folder_id,conversation_id,agent_type,position,is_active,is_pinned,created_at,updated_at) VALUES (1,3,'codex',0,1,0,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)")).await.unwrap();

        Migrator::up(&conn, None).await.unwrap();

        let provisional = conn.query_one(sql("SELECT lifecycle_state,branch_session_id,session_verified_at FROM conversation_branch WHERE branch_conversation_id=2")).await.unwrap().unwrap();
        assert_eq!(
            provisional
                .try_get::<String>("", "lifecycle_state")
                .unwrap(),
            "provisional"
        );
        assert!(provisional
            .try_get::<Option<String>>("", "branch_session_id")
            .unwrap()
            .is_none());
        assert!(provisional
            .try_get::<Option<String>>("", "session_verified_at")
            .unwrap()
            .is_none());

        let merged = conn.query_one(sql("SELECT b.lifecycle_state,c.status,c.pinned_at FROM conversation_branch b JOIN conversation c ON c.id=b.branch_conversation_id WHERE b.branch_conversation_id=3")).await.unwrap().unwrap();
        assert_eq!(
            merged.try_get::<String>("", "lifecycle_state").unwrap(),
            "merged"
        );
        assert_eq!(merged.try_get::<String>("", "status").unwrap(), "completed");
        assert!(merged
            .try_get::<Option<String>>("", "pinned_at")
            .unwrap()
            .is_none());
        assert!(conn
            .query_one(sql("SELECT id FROM opened_tab WHERE conversation_id=3"))
            .await
            .unwrap()
            .is_none());

        let incomplete_native = conn.query_one(sql("SELECT lifecycle_state,lifecycle_error FROM conversation_branch WHERE branch_conversation_id=4")).await.unwrap().unwrap();
        assert_eq!(
            incomplete_native
                .try_get::<String>("", "lifecycle_state")
                .unwrap(),
            "failed"
        );
        assert!(incomplete_native
            .try_get::<String>("", "lifecycle_error")
            .unwrap()
            .contains("no verified branch session"));
    }
}
