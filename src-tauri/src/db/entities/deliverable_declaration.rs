use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "deliverable_declaration")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub request_id: String,
    pub conversation_id: i32,
    pub turn_run_id: String,
    pub status: String,
    pub payload_json: String,
    pub outcome_json: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::conversation::Entity",
        from = "Column::ConversationId",
        to = "super::conversation::Column::Id"
    )]
    Conversation,
    #[sea_orm(
        belongs_to = "super::conversation_turn_run::Entity",
        from = "Column::TurnRunId",
        to = "super::conversation_turn_run::Column::Id"
    )]
    TurnRun,
}

impl Related<super::conversation::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Conversation.def()
    }
}

impl Related<super::conversation_turn_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::TurnRun.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
