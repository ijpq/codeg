use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ConversationBranch::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationBranch::BranchConversationId)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranch::SourceConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranch::SourceTitle)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranch::ForkMessageId)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranch::ForkMode)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranch::SnapshotContext)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranch::SnapshotConsumedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranch::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranch::LastMergedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranch::LastMergeKey)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranch::MergeTargetConversationId)
                            .integer()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_branch_source")
                    .table(ConversationBranch::Table)
                    .col(ConversationBranch::SourceConversationId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(ConversationBranchMerge::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationBranchMerge::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranchMerge::BranchConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranchMerge::SourceConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranchMerge::TargetConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranchMerge::Summary)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranchMerge::DeliverableIdsJson)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranchMerge::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranchMerge::ContextConsumedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_branch_merge_target_created")
                    .table(ConversationBranchMerge::Table)
                    .col(ConversationBranchMerge::TargetConversationId)
                    .col(ConversationBranchMerge::CreatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(ConversationBranchMerge::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationBranch::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ConversationBranch {
    Table,
    BranchConversationId,
    SourceConversationId,
    SourceTitle,
    ForkMessageId,
    ForkMode,
    SnapshotContext,
    SnapshotConsumedAt,
    CreatedAt,
    LastMergedAt,
    LastMergeKey,
    MergeTargetConversationId,
}

#[derive(DeriveIden)]
enum ConversationBranchMerge {
    Table,
    Id,
    BranchConversationId,
    SourceConversationId,
    TargetConversationId,
    Summary,
    DeliverableIdsJson,
    CreatedAt,
    ContextConsumedAt,
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
    use sea_orm_migration::MigratorTrait;

    use crate::db::migration::Migrator;

    fn sql(statement: &str) -> Statement {
        Statement::from_string(DbBackend::Sqlite, statement.to_owned())
    }

    #[tokio::test]
    async fn upgrades_existing_database_without_rewriting_conversations() {
        let conn = Database::connect("sqlite::memory:").await.expect("db");
        let migrations = <Migrator as MigratorTrait>::migrations();
        let branch_index = migrations
            .iter()
            .position(|migration| migration.name().contains("conversation_branch"))
            .expect("branch migration is registered");
        Migrator::up(&conn, Some(branch_index as u32))
            .await
            .expect("pre-branch schema");

        conn.execute(sql("INSERT INTO folder \
             (id, name, path, last_opened_at, created_at, updated_at, is_open, \
              sort_order, color, kind) \
             VALUES (901, 'Existing', '/tmp/existing', '2026-08-01 00:00:00', \
              '2026-08-01 00:00:00', '2026-08-01 00:00:00', 1, 0, 'inherit', \
              'regular')"))
            .await
            .expect("legacy folder");
        conn.execute(sql("INSERT INTO conversation \
             (id, folder_id, title, title_locked, agent_type, status, kind, \
              message_count, created_at, updated_at) \
             VALUES (901, 901, 'Existing conversation', 1, 'codex', 'completed', 'regular', 401, \
              '2026-08-01 00:00:00', '2026-08-01 00:00:00')"))
            .await
            .expect("legacy conversation");

        Migrator::up(&conn, None).await.expect("branch migration");

        let row = conn
            .query_one(sql(
                "SELECT title, message_count FROM conversation WHERE id = 901",
            ))
            .await
            .expect("query conversation")
            .expect("conversation remains");
        assert_eq!(
            row.try_get::<String>("", "title").unwrap(),
            "Existing conversation"
        );
        assert_eq!(row.try_get::<i32>("", "message_count").unwrap(), 401);
        for table in ["conversation_branch", "conversation_branch_merge"] {
            let row = conn
                .query_one(sql(&format!(
                    "SELECT COUNT(*) AS n FROM sqlite_master WHERE type='table' AND name='{table}'"
                )))
                .await
                .expect("query sqlite master")
                .expect("count row");
            assert_eq!(row.try_get::<i32>("", "n").unwrap(), 1);
        }
    }
}
