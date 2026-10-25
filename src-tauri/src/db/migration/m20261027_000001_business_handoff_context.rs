use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::Statement;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (table, column, definition) in [
            ("business_agent_switch", "phase", "TEXT"),
            (
                "business_conversation_state",
                "rule_context",
                "TEXT NOT NULL DEFAULT '{}'",
            ),
        ] {
            if !manager.has_column(table, column).await? {
                manager
                    .get_connection()
                    .execute(Statement::from_string(
                        manager.get_database_backend(),
                        format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                    ))
                    .await?;
            }
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Additive metadata remains readable after re-upgrade. Neither native
        // identities, history nor receipt status is rewritten on downgrade.
        Ok(())
    }
}
