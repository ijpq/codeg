use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_branch_creation_task")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub request_id: String,
    pub source_conversation_id: i32,
    pub fork_message_id: Option<String>,
    pub status: String,
    pub stage: String,
    pub result_json: Option<String>,
    pub error: Option<String>,
    pub cancel_requested: bool,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
