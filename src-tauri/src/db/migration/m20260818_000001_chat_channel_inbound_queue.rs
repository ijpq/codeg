use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ChatChannelTurnOrigin::Table)
                    .add_column(
                        ColumnDef::new(ChatChannelTurnOrigin::PromptJson)
                            .text()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ChatChannelTurnOrigin::Table)
                    .add_column(
                        ColumnDef::new(ChatChannelTurnOrigin::AttemptCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ChatChannelTurnOrigin::Table)
                    .add_column(
                        ColumnDef::new(ChatChannelTurnOrigin::LastError)
                            .text()
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
                    .table(ChatChannelTurnOrigin::Table)
                    .drop_column(ChatChannelTurnOrigin::LastError)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ChatChannelTurnOrigin::Table)
                    .drop_column(ChatChannelTurnOrigin::AttemptCount)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ChatChannelTurnOrigin::Table)
                    .drop_column(ChatChannelTurnOrigin::PromptJson)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ChatChannelTurnOrigin {
    Table,
    PromptJson,
    AttemptCount,
    LastError,
}
