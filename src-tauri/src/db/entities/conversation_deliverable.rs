use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_deliverable")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub conversation_id: i32,
    pub turn_run_id: Option<String>,
    pub root_path: String,
    pub path: String,
    pub kind: String,
    pub title: String,
    pub description: Option<String>,
    pub role: String,
    pub position: i32,
    pub source: String,
    pub size_bytes: Option<i64>,
    pub verified_at: DateTimeUtc,
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
