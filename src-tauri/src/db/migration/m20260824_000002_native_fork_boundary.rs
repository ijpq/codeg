use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            ColumnDef::new(ConversationBranch::SourceRolloutOffset)
                .big_integer()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::BranchRolloutOffset)
                .big_integer()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::ForkBoundaryKind)
                .string()
                .null()
                .to_owned(),
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(ConversationBranch::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            ConversationBranch::ForkBoundaryKind,
            ConversationBranch::BranchRolloutOffset,
            ConversationBranch::SourceRolloutOffset,
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
    SourceRolloutOffset,
    BranchRolloutOffset,
    ForkBoundaryKind,
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
    use sea_orm_migration::MigratorTrait;

    use crate::db::migration::Migrator;

    #[tokio::test]
    async fn adds_nullable_native_fork_boundaries_without_rewriting_old_branches() {
        let conn = Database::connect("sqlite::memory:").await.unwrap();
        let migrations = <Migrator as MigratorTrait>::migrations();
        let this = migrations
            .iter()
            .position(|migration| migration.name().contains("native_fork_boundary"))
            .unwrap();
        Migrator::up(&conn, Some(this as u32)).await.unwrap();
        conn.execute(Statement::from_string(
            DbBackend::Sqlite,
            "INSERT INTO folder (id,name,path,last_opened_at,created_at,updated_at,is_open,sort_order,color,kind) VALUES (1,'f','/tmp/f',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,1,0,'inherit','regular')".to_owned(),
        )).await.unwrap();
        for id in [1, 2] {
            conn.execute(Statement::from_string(DbBackend::Sqlite, format!("INSERT INTO conversation (id,folder_id,title,title_locked,agent_type,status,kind,message_count,created_at,updated_at) VALUES ({id},1,'c{id}',1,'codex','pending_review','regular',0,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)"))).await.unwrap();
        }
        conn.execute(Statement::from_string(DbBackend::Sqlite, "INSERT INTO conversation_branch (branch_conversation_id,source_conversation_id,fork_mode,inheritance_mode,inherited_message_count,inherited_context_chars,inherited_estimated_tokens,inheritance_compressed,inheritance_truncated,snapshot_version,lifecycle_state,created_at) VALUES (2,1,'native','native_fork',0,0,0,0,0,2,'ready',CURRENT_TIMESTAMP)".to_owned())).await.unwrap();

        Migrator::up(&conn, None).await.unwrap();
        let row = conn.query_one(Statement::from_string(DbBackend::Sqlite, "SELECT source_rollout_offset,branch_rollout_offset,fork_boundary_kind FROM conversation_branch WHERE branch_conversation_id=2".to_owned())).await.unwrap().unwrap();
        assert!(row
            .try_get::<Option<i64>>("", "source_rollout_offset")
            .unwrap()
            .is_none());
        assert!(row
            .try_get::<Option<i64>>("", "branch_rollout_offset")
            .unwrap()
            .is_none());
        assert!(row
            .try_get::<Option<String>>("", "fork_boundary_kind")
            .unwrap()
            .is_none());
    }
}
