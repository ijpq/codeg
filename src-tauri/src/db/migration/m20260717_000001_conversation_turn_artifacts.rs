use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ConversationTurnRun::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationTurnRun::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnRun::ConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnRun::ConnectionId)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ConversationTurnRun::ClientMessageId).string().null())
                    .col(ColumnDef::new(ConversationTurnRun::FolderId).integer().null())
                    // Snapshot the actual root used by this turn. Folder rows can
                    // later move or be soft-deleted, while an artifact still needs
                    // an unambiguous path for display/opening.
                    .col(
                        ColumnDef::new(ConversationTurnRun::RootPath)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnRun::Status)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnRun::CaptureIncomplete)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(ConversationTurnRun::StopReason).string().null())
                    .col(
                        ColumnDef::new(ConversationTurnRun::StartedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnRun::CompletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                ConversationTurnRun::Table,
                                ConversationTurnRun::ConversationId,
                            )
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_turn_run_conversation_started")
                    .table(ConversationTurnRun::Table)
                    .col(ConversationTurnRun::ConversationId)
                    .col(ConversationTurnRun::StartedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_turn_run_connection_status")
                    .table(ConversationTurnRun::Table)
                    .col(ConversationTurnRun::ConnectionId)
                    .col(ConversationTurnRun::Status)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ConversationTurnFileChange::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationTurnFileChange::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnFileChange::TurnRunId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnFileChange::Path)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ConversationTurnFileChange::OldPath).text().null())
                    .col(
                        ColumnDef::new(ConversationTurnFileChange::Kind)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnFileChange::Source)
                            .string()
                            .not_null()
                            .default("watcher"),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnFileChange::Attribution)
                            .string()
                            .not_null()
                            .default("exclusive"),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnFileChange::FirstSeenAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnFileChange::LastSeenAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnFileChange::EventCount)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    // NULL while a turn is live; finalized to true/false on its
                    // terminal event. Keeping this nullable lets the UI distinguish
                    // "not checked yet" from "confirmed removed".
                    .col(
                        ColumnDef::new(ConversationTurnFileChange::FinalExists)
                            .boolean()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnFileChange::SizeBytes)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationTurnFileChange::ModifiedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                ConversationTurnFileChange::Table,
                                ConversationTurnFileChange::TurnRunId,
                            )
                            .to(ConversationTurnRun::Table, ConversationTurnRun::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_turn_file_change_run_path")
                    .table(ConversationTurnFileChange::Table)
                    .col(ConversationTurnFileChange::TurnRunId)
                    .col(ConversationTurnFileChange::Path)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(ConversationTurnFileChange::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationTurnRun::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum ConversationTurnRun {
    Table,
    Id,
    ConversationId,
    ConnectionId,
    ClientMessageId,
    FolderId,
    RootPath,
    Status,
    CaptureIncomplete,
    StopReason,
    StartedAt,
    CompletedAt,
}

#[derive(DeriveIden)]
enum ConversationTurnFileChange {
    Table,
    Id,
    TurnRunId,
    Path,
    OldPath,
    Kind,
    Source,
    Attribution,
    FirstSeenAt,
    LastSeenAt,
    EventCount,
    FinalExists,
    SizeBytes,
    ModifiedAt,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    Id,
}
