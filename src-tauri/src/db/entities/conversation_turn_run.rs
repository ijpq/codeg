use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum ConversationTurnRunStatus {
    #[sea_orm(string_value = "running")]
    Running,
    #[sea_orm(string_value = "completed")]
    Completed,
    #[sea_orm(string_value = "cancelled")]
    Cancelled,
    #[sea_orm(string_value = "interrupted")]
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_turn_run")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub conversation_id: i32,
    pub connection_id: String,
    pub client_message_id: Option<String>,
    pub folder_id: Option<i32>,
    pub root_path: String,
    pub status: ConversationTurnRunStatus,
    pub capture_incomplete: bool,
    pub stop_reason: Option<String>,
    pub started_at: DateTimeUtc,
    pub completed_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::conversation::Entity",
        from = "Column::ConversationId",
        to = "super::conversation::Column::Id"
    )]
    Conversation,
    #[sea_orm(has_many = "super::conversation_turn_file_change::Entity")]
    FileChanges,
    #[sea_orm(has_many = "super::conversation_deliverable::Entity")]
    Deliverables,
}

impl Related<super::conversation::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Conversation.def()
    }
}

impl Related<super::conversation_turn_file_change::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::FileChanges.def()
    }
}

impl Related<super::conversation_deliverable::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Deliverables.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
