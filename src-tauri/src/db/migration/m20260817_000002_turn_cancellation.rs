use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            ColumnDef::new(ConversationTurnRun::CancelRequestId)
                .string()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationTurnRun::CancelRequestedAt)
                .timestamp_with_time_zone()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationTurnRun::CancelDeadlineAt)
                .timestamp_with_time_zone()
                .null()
                .to_owned(),
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(ConversationTurnRun::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }
        manager
            .create_index(
                Index::create()
                    .name("idx_turn_run_cancel_deadline")
                    .table(ConversationTurnRun::Table)
                    .col(ConversationTurnRun::Status)
                    .col(ConversationTurnRun::CancelDeadlineAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_turn_run_cancel_deadline")
                    .to_owned(),
            )
            .await?;
        for column in [
            ConversationTurnRun::CancelDeadlineAt,
            ConversationTurnRun::CancelRequestedAt,
            ConversationTurnRun::CancelRequestId,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(ConversationTurnRun::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum ConversationTurnRun {
    Table,
    Status,
    CancelRequestId,
    CancelRequestedAt,
    CancelDeadlineAt,
}
