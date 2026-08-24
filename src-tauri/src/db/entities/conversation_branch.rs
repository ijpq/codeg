use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_branch")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub branch_conversation_id: i32,
    pub creation_request_id: Option<String>,
    pub operation_id: Option<String>,
    pub source_conversation_id: i32,
    pub source_title: Option<String>,
    pub fork_message_id: Option<String>,
    pub fork_mode: String,
    pub source_session_id: Option<String>,
    pub branch_session_id: Option<String>,
    pub inheritance_mode: String,
    pub inherited_message_count: i32,
    pub inherited_context_chars: i64,
    pub inherited_estimated_tokens: i64,
    pub inheritance_compressed: bool,
    pub inheritance_truncated: bool,
    pub inheritance_note: Option<String>,
    pub forked_through_at: Option<DateTimeUtc>,
    pub snapshot_version: i32,
    pub snapshot_images_json: Option<String>,
    pub snapshot_context: Option<String>,
    pub snapshot_consumed_at: Option<DateTimeUtc>,
    pub lifecycle_state: String,
    pub lifecycle_error: Option<String>,
    pub lifecycle_updated_at: Option<DateTimeUtc>,
    pub session_verified_at: Option<DateTimeUtc>,
    pub first_prompt_client_message_id: Option<String>,
    pub first_prompt_queued_at: Option<DateTimeUtc>,
    pub first_prompt_accepted_at: Option<DateTimeUtc>,
    pub initialization_retry_count: i32,
    pub last_connection_id: Option<String>,
    pub snapshot_digest: Option<String>,
    pub created_at: DateTimeUtc,
    pub last_merged_at: Option<DateTimeUtc>,
    pub last_merge_key: Option<String>,
    pub merge_target_conversation_id: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
