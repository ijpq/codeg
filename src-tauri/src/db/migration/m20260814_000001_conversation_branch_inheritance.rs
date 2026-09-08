use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::Statement;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let columns = [
            ColumnDef::new(ConversationBranch::SourceSessionId)
                .text()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::BranchSessionId)
                .text()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::InheritanceMode)
                .string()
                .not_null()
                .default("structured_snapshot")
                .to_owned(),
            ColumnDef::new(ConversationBranch::InheritedMessageCount)
                .integer()
                .not_null()
                .default(0)
                .to_owned(),
            ColumnDef::new(ConversationBranch::InheritedContextChars)
                .big_integer()
                .not_null()
                .default(0)
                .to_owned(),
            ColumnDef::new(ConversationBranch::InheritedEstimatedTokens)
                .big_integer()
                .not_null()
                .default(0)
                .to_owned(),
            ColumnDef::new(ConversationBranch::InheritanceCompressed)
                .boolean()
                .not_null()
                .default(false)
                .to_owned(),
            ColumnDef::new(ConversationBranch::InheritanceTruncated)
                .boolean()
                .not_null()
                .default(false)
                .to_owned(),
            ColumnDef::new(ConversationBranch::InheritanceNote)
                .text()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::ForkedThroughAt)
                .timestamp_with_time_zone()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationBranch::SnapshotVersion)
                .integer()
                .not_null()
                .default(1)
                .to_owned(),
            ColumnDef::new(ConversationBranch::SnapshotImagesJson)
                .text()
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

        // Existing relations remain valid and are only annotated. No source or
        // branch transcript is rewritten. Session ids can be recovered from the
        // durable conversation rows when present.
        let backend = manager.get_database_backend();
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                "UPDATE conversation_branch SET \
                 inheritance_mode = CASE WHEN fork_mode = 'native' THEN 'native_fork' ELSE 'structured_snapshot' END, \
                 source_session_id = (SELECT external_id FROM conversation WHERE conversation.id = conversation_branch.source_conversation_id), \
                 branch_session_id = (SELECT external_id FROM conversation WHERE conversation.id = conversation_branch.branch_conversation_id), \
                 inheritance_note = 'Legacy branch: detailed inheritance counts were not recorded by its creator.'"
                    .to_owned(),
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            ConversationBranch::SnapshotVersion,
            ConversationBranch::SnapshotImagesJson,
            ConversationBranch::ForkedThroughAt,
            ConversationBranch::InheritanceNote,
            ConversationBranch::InheritanceTruncated,
            ConversationBranch::InheritanceCompressed,
            ConversationBranch::InheritedEstimatedTokens,
            ConversationBranch::InheritedContextChars,
            ConversationBranch::InheritedMessageCount,
            ConversationBranch::InheritanceMode,
            ConversationBranch::BranchSessionId,
            ConversationBranch::SourceSessionId,
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
    SourceSessionId,
    BranchSessionId,
    InheritanceMode,
    InheritedMessageCount,
    InheritedContextChars,
    InheritedEstimatedTokens,
    InheritanceCompressed,
    InheritanceTruncated,
    InheritanceNote,
    ForkedThroughAt,
    SnapshotVersion,
    SnapshotImagesJson,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

    #[tokio::test]
    async fn upgrades_legacy_branch_without_changing_conversations() {
        let conn = Database::connect("sqlite::memory:").await.unwrap();
        conn.execute_unprepared(
            "CREATE TABLE conversation (id INTEGER PRIMARY KEY, title TEXT, external_id TEXT);\
             CREATE TABLE conversation_branch (\
               branch_conversation_id INTEGER PRIMARY KEY, source_conversation_id INTEGER NOT NULL,\
               source_title TEXT, fork_message_id TEXT, fork_mode TEXT NOT NULL,\
               snapshot_context TEXT, snapshot_consumed_at TEXT, created_at TEXT NOT NULL,\
               last_merged_at TEXT, last_merge_key TEXT, merge_target_conversation_id INTEGER);\
             INSERT INTO conversation VALUES (1, 'source', 'source-session');\
             INSERT INTO conversation VALUES (2, 'branch', 'branch-session');\
             INSERT INTO conversation_branch \
               (branch_conversation_id, source_conversation_id, fork_mode, created_at)\
               VALUES (2, 1, 'snapshot', '2026-08-13 00:00:00');",
        )
        .await
        .unwrap();

        Migration.up(&SchemaManager::new(&conn)).await.unwrap();

        let row = conn
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT inheritance_mode, source_session_id, branch_session_id, snapshot_version \
                 FROM conversation_branch WHERE branch_conversation_id = 2"
                    .to_owned(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.try_get::<String>("", "inheritance_mode").unwrap(),
            "structured_snapshot"
        );
        assert_eq!(
            row.try_get::<String>("", "source_session_id").unwrap(),
            "source-session"
        );
        assert_eq!(
            row.try_get::<String>("", "branch_session_id").unwrap(),
            "branch-session"
        );
        assert_eq!(row.try_get::<i32>("", "snapshot_version").unwrap(), 1);
        assert_eq!(
            conn.query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT title FROM conversation WHERE id = 1".to_owned(),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<String>("", "title")
            .unwrap(),
            "source"
        );
    }
}
