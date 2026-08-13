use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_branch")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub branch_conversation_id: i32,
    pub source_conversation_id: i32,
    pub source_title: Option<String>,
    pub fork_message_id: Option<String>,
    pub fork_mode: String,
    pub snapshot_context: Option<String>,
    pub snapshot_consumed_at: Option<DateTimeUtc>,
    pub created_at: DateTimeUtc,
    pub last_merged_at: Option<DateTimeUtc>,
    pub last_merge_key: Option<String>,
    pub merge_target_conversation_id: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
