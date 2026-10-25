use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Keep conversation_deliverable as the deduplicated, durable fact row
        // for a path while enriching it with the stat/validity metadata needed
        // by the UI and by ID-scoped file operations.
        for column in [
            ColumnDef::new(ConversationDeliverable::FileName)
                .text()
                .not_null()
                .default("")
                .to_owned(),
            ColumnDef::new(ConversationDeliverable::Extension)
                .string()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationDeliverable::ModifiedAt)
                .timestamp_with_time_zone()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationDeliverable::IsValid)
                .boolean()
                .not_null()
                .default(true)
                .to_owned(),
            ColumnDef::new(ConversationDeliverable::InvalidReason)
                .string()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationDeliverable::IsHidden)
                .boolean()
                .not_null()
                .default(false)
                .to_owned(),
            ColumnDef::new(ConversationDeliverable::LastCheckedAt)
                .timestamp_with_time_zone()
                .null()
                .to_owned(),
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(ConversationDeliverable::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }

        // An explicit empty declaration is semantically meaningful: it blocks
        // fallback inference for the turn. Persist that fact on the run even
        // when no association rows are produced.
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationTurnRun::Table)
                    .add_column(
                        ColumnDef::new(ConversationTurnRun::DeliverablesDeclaredAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationTurnRun::Table)
                    .add_column(
                        ColumnDef::new(ConversationTurnRun::InputPathsJson)
                            .text()
                            .not_null()
                            .default("[]"),
                    )
                    .to_owned(),
            )
            .await?;

        // One aggregate deliverable may belong to several turns. Association
        // rows retain per-turn role/source/order while the aggregate table stays
        // deduplicated by conversation + canonical root + relative path.
        manager
            .create_table(
                Table::create()
                    .table(ConversationTurnDeliverable::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationTurnDeliverable::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnDeliverable::ConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnDeliverable::TurnRunId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnDeliverable::DeliverableId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnDeliverable::Source)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnDeliverable::Title)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnDeliverable::Description)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnDeliverable::Role)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnDeliverable::Position)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnDeliverable::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnDeliverable::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                ConversationTurnDeliverable::Table,
                                ConversationTurnDeliverable::ConversationId,
                            )
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                ConversationTurnDeliverable::Table,
                                ConversationTurnDeliverable::TurnRunId,
                            )
                            .to(ConversationTurnRun::Table, ConversationTurnRun::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                ConversationTurnDeliverable::Table,
                                ConversationTurnDeliverable::DeliverableId,
                            )
                            .to(ConversationDeliverable::Table, ConversationDeliverable::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_turn_deliverable_run_item")
                    .table(ConversationTurnDeliverable::Table)
                    .col(ConversationTurnDeliverable::TurnRunId)
                    .col(ConversationTurnDeliverable::DeliverableId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_turn_deliverable_conversation_created")
                    .table(ConversationTurnDeliverable::Table)
                    .col(ConversationTurnDeliverable::ConversationId)
                    .col(ConversationTurnDeliverable::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // Backfill existing 0.21.2 declarations. Rows without a turn id remain
        // valid conversation-level legacy facts but cannot be fabricated into a
        // specific historical turn.
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE conversation_deliverable
                 SET source = CASE WHEN source = 'agent_declared' THEN 'declared' ELSE source END,
                     file_name = path",
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "INSERT OR IGNORE INTO conversation_turn_deliverable
                   (id, conversation_id, turn_run_id, deliverable_id, source, title, description, role, position, created_at, updated_at)
                 SELECT lower(hex(randomblob(16))), conversation_id, turn_run_id, id,
                        CASE WHEN source = 'agent_declared' THEN 'declared' ELSE source END,
                        title, description, role, position, created_at, updated_at
                 FROM conversation_deliverable
                 WHERE turn_run_id IS NOT NULL",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(ConversationTurnDeliverable::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationTurnRun::Table)
                    .drop_column(ConversationTurnRun::InputPathsJson)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationTurnRun::Table)
                    .drop_column(ConversationTurnRun::DeliverablesDeclaredAt)
                    .to_owned(),
            )
            .await?;
        for column in [
            ConversationDeliverable::LastCheckedAt,
            ConversationDeliverable::IsHidden,
            ConversationDeliverable::InvalidReason,
            ConversationDeliverable::IsValid,
            ConversationDeliverable::ModifiedAt,
            ConversationDeliverable::Extension,
            ConversationDeliverable::FileName,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(ConversationDeliverable::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum ConversationDeliverable {
    Table,
    Id,
    FileName,
    Extension,
    ModifiedAt,
    IsValid,
    InvalidReason,
    IsHidden,
    LastCheckedAt,
}

#[derive(DeriveIden)]
enum ConversationTurnRun {
    Table,
    Id,
    DeliverablesDeclaredAt,
    InputPathsJson,
}

#[derive(DeriveIden)]
enum ConversationTurnDeliverable {
    Table,
    Id,
    ConversationId,
    TurnRunId,
    DeliverableId,
    Source,
    Title,
    Description,
    Role,
    Position,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    Id,
}
