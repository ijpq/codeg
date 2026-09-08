use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationTurnRun::Table)
                    .add_column(
                        ColumnDef::new(ConversationTurnRun::PromptFingerprint)
                            .string()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationTurnRun::Table)
                    .drop_column(ConversationTurnRun::PromptFingerprint)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ConversationTurnRun {
    Table,
    PromptFingerprint,
}
