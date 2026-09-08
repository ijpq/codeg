use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum ConversationTurnRunStatus {
    #[sea_orm(string_value = "running")]
    Running,
    #[sea_orm(string_value = "cancelling")]
    Cancelling,
    #[sea_orm(string_value = "completed")]
    Completed,
    #[sea_orm(string_value = "cancelled")]
    Cancelled,
    #[sea_orm(string_value = "interrupted")]
    Interrupted,
    #[sea_orm(string_value = "failed")]
    Failed,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_turn_run")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub conversation_id: i32,
    pub connection_id: String,
    pub client_message_id: Option<String>,
    /// Set only after the prompt command has been accepted by the live ACP
    /// connection. This is the durable idempotency receipt used across browser
    /// reloads and replacement connections.
    pub prompt_accepted_at: Option<DateTimeUtc>,
    /// SHA-256 of the normalized user-visible prompt blocks. This lets a
    /// reloaded viewer bind the run to the parser's durable user-turn id
    /// without relying on the sender-only optimistic id or wall-clock guesses.
    pub prompt_fingerprint: Option<String>,
    /// Stable id of the first accepted cancellation request. Repeated cancel
    /// calls read this receipt instead of notifying the agent again.
    pub cancel_request_id: Option<String>,
    pub cancel_requested_at: Option<DateTimeUtc>,
    pub cancel_deadline_at: Option<DateTimeUtc>,
    pub folder_id: Option<i32>,
    pub root_path: String,
    pub status: ConversationTurnRunStatus,
    pub capture_incomplete: bool,
    pub stop_reason: Option<String>,
    pub started_at: DateTimeUtc,
    pub completed_at: Option<DateTimeUtc>,
    pub deliverables_declared_at: Option<DateTimeUtc>,
    pub input_paths_json: String,
    pub declaration_status: String,
    pub declaration_attempted_at: Option<DateTimeUtc>,
    pub declaration_error: Option<String>,
    pub expectation_json: String,
    pub settlement_status: String,
    pub settled_at: Option<DateTimeUtc>,
    pub missing_expected_paths_json: String,
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
    #[sea_orm(has_many = "super::conversation_turn_deliverable::Entity")]
    DeliverableAssociations,
    #[sea_orm(has_many = "super::deliverable_declaration::Entity")]
    Declarations,
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

impl Related<super::conversation_turn_deliverable::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::DeliverableAssociations.def()
    }
}

impl Related<super::deliverable_declaration::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Declarations.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
