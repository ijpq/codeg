use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::Statement;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager
            .has_column("conversation_agent_session", "delivered_rule_digest")
            .await?
        {
            manager.get_connection().execute(Statement::from_string(
                manager.get_database_backend(),
                "ALTER TABLE conversation_agent_session ADD COLUMN delivered_rule_digest TEXT NOT NULL DEFAULT ''".to_owned(),
            )).await?;
        }
        Ok(())
    }
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Additive delivery metadata is ignored by older versions. Preserve
        // native sessions, shared history and the original rule snapshots.
        Ok(())
    }
}
