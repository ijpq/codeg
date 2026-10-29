use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::Statement;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager
            .has_column("business_message", "context_digest")
            .await?
        {
            manager.get_connection().execute(Statement::from_string(
                manager.get_database_backend(),
                "ALTER TABLE business_message ADD COLUMN context_digest TEXT NOT NULL DEFAULT ''".to_owned(),
            )).await?;
        }
        manager.get_connection().execute(Statement::from_string(
            manager.get_database_backend(),
            "CREATE TABLE IF NOT EXISTS business_history_delivery (
                session_id INTEGER NOT NULL REFERENCES conversation_agent_session(id) ON DELETE CASCADE,
                sequence INTEGER NOT NULL REFERENCES business_message(sequence) ON DELETE CASCADE,
                digest TEXT NOT NULL, total_bytes INTEGER NOT NULL,
                ranges_json TEXT NOT NULL, complete INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(session_id,sequence))".to_owned(),
        )).await?;
        if !manager
            .has_column("business_prompt_receipt", "history_delivery_json")
            .await?
        {
            manager.get_connection().execute(Statement::from_string(
                manager.get_database_backend(),
                "ALTER TABLE business_prompt_receipt ADD COLUMN history_delivery_json TEXT NOT NULL DEFAULT '[]'".to_owned(),
            )).await?;
        }
        if !manager
            .has_column("conversation_agent_session", "history_sequence")
            .await?
        {
            manager.get_connection().execute(Statement::from_string(
                manager.get_database_backend(),
                "ALTER TABLE conversation_agent_session ADD COLUMN history_sequence INTEGER NOT NULL DEFAULT 0".to_owned(),
            )).await?;
        }
        // Invalidation is part of the source UPSERT, even when its caller is
        // outside a larger transaction. Acceptance cannot race the prose write.
        manager.get_connection().execute(Statement::from_string(
            manager.get_database_backend(),
            "CREATE TRIGGER IF NOT EXISTS business_history_prose_changed AFTER UPDATE OF context_digest ON business_message WHEN OLD.context_digest<>NEW.context_digest BEGIN
                DELETE FROM business_history_delivery WHERE sequence=NEW.sequence;
                UPDATE conversation_agent_session SET history_sequence=MIN(history_sequence,NEW.sequence-1) WHERE conversation_id=NEW.conversation_id;
            END".to_owned(),
        )).await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Older binaries ignore this additive acknowledgement metadata. Keep
        // it for re-upgrade; never rewrite or remove native/business history.
        Ok(())
    }
}
