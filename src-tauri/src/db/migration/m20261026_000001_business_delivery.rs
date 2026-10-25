use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::Statement;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "CREATE TABLE IF NOT EXISTS business_delivery (
              id TEXT PRIMARY KEY, conversation_id INTEGER NOT NULL REFERENCES conversation(id) ON DELETE CASCADE,
              client_message_id TEXT NOT NULL, request_json TEXT NOT NULL,
              target_agent TEXT NOT NULL, target_external_id TEXT NOT NULL, target_generation TEXT NOT NULL,
              submitted_revision INTEGER NOT NULL, mode TEXT NOT NULL, target_settings TEXT NOT NULL DEFAULT '{}',
              status TEXT NOT NULL DEFAULT 'queued', error TEXT, connection_id TEXT, task_id TEXT,
              deadline_at TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
              updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
              UNIQUE(conversation_id,client_message_id))",
            "CREATE INDEX IF NOT EXISTS idx_business_delivery_pending ON business_delivery(status,conversation_id,created_at)",
        ] {
            manager.get_connection().execute(Statement::from_string(manager.get_database_backend(), sql.to_owned())).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Preserve receipts and payloads for auditing and later upgrade. A downgrade
        // must not silently deliver work that the older binary cannot supervise.
        manager.get_connection().execute(Statement::from_string(manager.get_database_backend(),
            "UPDATE business_delivery SET status='suspended',error='SCHEMA_DOWNGRADED',updated_at=CURRENT_TIMESTAMP WHERE status IN ('queued','delivering','delivered','running','cancelling')".to_owned())).await?;
        Ok(())
    }
}
