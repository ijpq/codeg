use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            ColumnDef::new(ConversationTurnRun::DeclarationStatus)
                .string()
                .not_null()
                .default("not_called")
                .to_owned(),
            ColumnDef::new(ConversationTurnRun::DeclarationAttemptedAt)
                .timestamp_with_time_zone()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationTurnRun::DeclarationError)
                .text()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationTurnRun::ExpectationJson)
                .text()
                .not_null()
                .default(
                    r#"{"publish_required":true,"expects_code_changes":false,"requested_paths":[]}"#,
                )
                .to_owned(),
            ColumnDef::new(ConversationTurnRun::SettlementStatus)
                .string()
                .not_null()
                .default("pending")
                .to_owned(),
            ColumnDef::new(ConversationTurnRun::SettledAt)
                .timestamp_with_time_zone()
                .null()
                .to_owned(),
            ColumnDef::new(ConversationTurnRun::MissingExpectedPathsJson)
                .text()
                .not_null()
                .default("[]")
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

        for column in [
            ColumnDef::new(ConversationDeliverable::Category)
                .string()
                .not_null()
                .default("standalone_output")
                .to_owned(),
            ColumnDef::new(ConversationDeliverable::ChangeKind)
                .string()
                .not_null()
                .default("created")
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

        for column in [
            ColumnDef::new(ConversationTurnDeliverable::Category)
                .string()
                .not_null()
                .default("standalone_output")
                .to_owned(),
            ColumnDef::new(ConversationTurnDeliverable::ChangeKind)
                .string()
                .not_null()
                .default("created")
                .to_owned(),
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(ConversationTurnDeliverable::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }

        manager
            .create_table(
                Table::create()
                    .table(DeliverableDeclaration::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DeliverableDeclaration::RequestId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(DeliverableDeclaration::ConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeliverableDeclaration::TurnRunId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeliverableDeclaration::Status)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeliverableDeclaration::PayloadJson)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeliverableDeclaration::OutcomeJson)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeliverableDeclaration::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeliverableDeclaration::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                DeliverableDeclaration::Table,
                                DeliverableDeclaration::ConversationId,
                            )
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                DeliverableDeclaration::Table,
                                DeliverableDeclaration::TurnRunId,
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
                    .name("idx_deliverable_declaration_turn_created")
                    .table(DeliverableDeclaration::Table)
                    .col(DeliverableDeclaration::TurnRunId)
                    .col(DeliverableDeclaration::CreatedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(DeliverableDeclaration::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;

        for column in [
            ConversationTurnDeliverable::ChangeKind,
            ConversationTurnDeliverable::Category,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(ConversationTurnDeliverable::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        for column in [
            ConversationDeliverable::ChangeKind,
            ConversationDeliverable::Category,
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
        for column in [
            ConversationTurnRun::MissingExpectedPathsJson,
            ConversationTurnRun::SettledAt,
            ConversationTurnRun::SettlementStatus,
            ConversationTurnRun::ExpectationJson,
            ConversationTurnRun::DeclarationError,
            ConversationTurnRun::DeclarationAttemptedAt,
            ConversationTurnRun::DeclarationStatus,
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
    Id,
    DeclarationStatus,
    DeclarationAttemptedAt,
    DeclarationError,
    ExpectationJson,
    SettlementStatus,
    SettledAt,
    MissingExpectedPathsJson,
}

#[derive(DeriveIden)]
enum ConversationDeliverable {
    Table,
    Category,
    ChangeKind,
}

#[derive(DeriveIden)]
enum ConversationTurnDeliverable {
    Table,
    Category,
    ChangeKind,
}

#[derive(DeriveIden)]
enum DeliverableDeclaration {
    Table,
    RequestId,
    ConversationId,
    TurnRunId,
    Status,
    PayloadJson,
    OutcomeJson,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    Id,
}
