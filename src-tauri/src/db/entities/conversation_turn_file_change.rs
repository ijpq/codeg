use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum ConversationTurnFileChangeKind {
    #[sea_orm(string_value = "created")]
    Created,
    #[sea_orm(string_value = "modified")]
    Modified,
    #[sea_orm(string_value = "deleted")]
    Deleted,
    #[sea_orm(string_value = "renamed")]
    Renamed,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_turn_file_change")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub turn_run_id: String,
    pub path: String,
    pub old_path: Option<String>,
    pub kind: ConversationTurnFileChangeKind,
    pub source: String,
    pub attribution: String,
    pub first_seen_at: DateTimeUtc,
    pub last_seen_at: DateTimeUtc,
    pub event_count: i32,
    pub final_exists: Option<bool>,
    pub size_bytes: Option<i64>,
    pub modified_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::conversation_turn_run::Entity",
        from = "Column::TurnRunId",
        to = "super::conversation_turn_run::Column::Id"
    )]
    TurnRun,
}

impl Related<super::conversation_turn_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::TurnRun.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
