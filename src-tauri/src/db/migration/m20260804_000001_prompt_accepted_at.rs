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
                        ColumnDef::new(ConversationTurnRun::PromptAcceptedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_turn_run_conversation_client_accepted")
                    .table(ConversationTurnRun::Table)
                    .col(ConversationTurnRun::ConversationId)
                    .col(ConversationTurnRun::ClientMessageId)
                    .col(ConversationTurnRun::PromptAcceptedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_turn_run_conversation_client_accepted")
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationTurnRun::Table)
                    .drop_column(ConversationTurnRun::PromptAcceptedAt)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ConversationTurnRun {
    Table,
    ConversationId,
    ClientMessageId,
    PromptAcceptedAt,
}
