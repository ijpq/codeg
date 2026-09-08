use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ConversationBranchCreationTask::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationBranchCreationTask::RequestId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranchCreationTask::SourceConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ConversationBranchCreationTask::ForkMessageId).string())
                    .col(
                        ColumnDef::new(ConversationBranchCreationTask::Status)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranchCreationTask::Stage)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ConversationBranchCreationTask::ResultJson).text())
                    .col(ColumnDef::new(ConversationBranchCreationTask::Error).text())
                    .col(
                        ColumnDef::new(ConversationBranchCreationTask::CancelRequested)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(ConversationBranchCreationTask::WorkerInstanceId).string())
                    .col(
                        ColumnDef::new(ConversationBranchCreationTask::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationBranchCreationTask::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                ConversationBranchCreationTask::Table,
                                ConversationBranchCreationTask::SourceConversationId,
                            )
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(ConversationBranchCreationTask::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ConversationBranchCreationTask {
    Table,
    RequestId,
    SourceConversationId,
    ForkMessageId,
    Status,
    Stage,
    ResultJson,
    Error,
    CancelRequested,
    WorkerInstanceId,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    Id,
}
