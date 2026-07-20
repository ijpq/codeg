use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ConversationDeliverable::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationDeliverable::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::ConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::TurnRunId)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::RootPath)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::Path)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::Kind)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::Title)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::Description)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::Role)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::Position)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::Source)
                            .string()
                            .not_null()
                            .default("agent_declared"),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::SizeBytes)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::VerifiedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationDeliverable::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                ConversationDeliverable::Table,
                                ConversationDeliverable::ConversationId,
                            )
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                ConversationDeliverable::Table,
                                ConversationDeliverable::TurnRunId,
                            )
                            .to(ConversationTurnRun::Table, ConversationTurnRun::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_deliverable_conversation_path")
                    .table(ConversationDeliverable::Table)
                    .col(ConversationDeliverable::ConversationId)
                    .col(ConversationDeliverable::RootPath)
                    .col(ConversationDeliverable::Path)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_deliverable_conversation_position")
                    .table(ConversationDeliverable::Table)
                    .col(ConversationDeliverable::ConversationId)
                    .col(ConversationDeliverable::Position)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(ConversationDeliverable::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ConversationDeliverable {
    Table,
    Id,
    ConversationId,
    TurnRunId,
    RootPath,
    Path,
    Kind,
    Title,
    Description,
    Role,
    Position,
    Source,
    SizeBytes,
    VerifiedAt,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum ConversationTurnRun {
    Table,
    Id,
}
