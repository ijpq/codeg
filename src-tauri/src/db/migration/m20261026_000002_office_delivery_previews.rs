use sea_orm_migration::{prelude::*, sea_orm::Statement};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute(Statement::from_string(manager.get_database_backend(),
            "CREATE TABLE IF NOT EXISTS office_delivery_preview (
              turn_run_id TEXT PRIMARY KEY REFERENCES conversation_turn_run(id) ON DELETE CASCADE,
              conversation_id INTEGER NOT NULL REFERENCES conversation(id) ON DELETE CASCADE,
              deliverable_id TEXT, file_name TEXT, original_path TEXT, snapshot_path TEXT, sha256 TEXT,
              status TEXT NOT NULL, error TEXT, completed_at TEXT NOT NULL,
              created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)".to_owned())).await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Preview metadata/cache is additive. Older binaries ignore it; retain
        // immutable review copies across an application rollback.
        Ok(())
    }
}
