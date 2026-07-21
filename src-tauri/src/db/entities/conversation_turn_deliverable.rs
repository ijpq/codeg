use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_turn_deliverable")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub conversation_id: i32,
    pub turn_run_id: String,
    pub deliverable_id: String,
    pub source: String,
    pub title: String,
    pub description: Option<String>,
    pub role: String,
    pub position: i32,
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
    #[sea_orm(
        belongs_to = "super::conversation_deliverable::Entity",
        from = "Column::DeliverableId",
        to = "super::conversation_deliverable::Column::Id"
    )]
    Deliverable,
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

impl Related<super::conversation_deliverable::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Deliverable.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
