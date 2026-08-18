use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "chat_channel_turn_origin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub channel_id: i32,
    pub sender_id: String,
    pub conversation_id: i32,
    pub connection_id: Option<String>,
    pub origin_message_id: String,
    pub client_message_id: String,
    pub turn_run_id: Option<String>,
    pub target_json: String,
    /// Serialized `Vec<PromptInputBlock>` retained until dispatch succeeds.
    /// This is the durable inbound queue used across ACP/CodeG restarts.
    pub prompt_json: Option<String>,
    pub status: String,
    pub attempt_count: i32,
    pub last_error: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    pub final_captured_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::chat_channel::Entity",
        from = "Column::ChannelId",
        to = "super::chat_channel::Column::Id"
    )]
    ChatChannel,
    #[sea_orm(has_many = "super::chat_channel_outbox::Entity")]
    Outbox,
}

impl Related<super::chat_channel::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ChatChannel.def()
    }
}

impl Related<super::chat_channel_outbox::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Outbox.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
