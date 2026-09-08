use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_branch_merge")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub branch_conversation_id: i32,
    pub source_conversation_id: i32,
    pub target_conversation_id: i32,
    pub summary: String,
    pub deliverable_ids_json: String,
    pub created_at: DateTimeUtc,
    pub context_consumed_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
