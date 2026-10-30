use sea_orm_migration::{prelude::*, sea_orm::Statement};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "CREATE TABLE IF NOT EXISTS conversation_compaction (
                conversation_id INTEGER NOT NULL REFERENCES conversation(id) ON DELETE CASCADE,
                external_id TEXT NOT NULL, compaction_id TEXT NOT NULL,
                connection_id TEXT NOT NULL, turn_run_id TEXT,
                started_at_ms INTEGER NOT NULL, last_seq INTEGER NOT NULL,
                status TEXT NOT NULL, meta_json TEXT NOT NULL,
                PRIMARY KEY(conversation_id,external_id,compaction_id))",
            "CREATE INDEX IF NOT EXISTS idx_compaction_window ON conversation_compaction(conversation_id,started_at_ms)",
        ] {
            manager.get_connection().execute(Statement::from_string(
                manager.get_database_backend(), sql.to_owned(),
            )).await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Additive display metadata stays available on re-upgrade. Older
        // binaries ignore it; rollback must not delete user-visible history.
        Ok(())
    }
}
