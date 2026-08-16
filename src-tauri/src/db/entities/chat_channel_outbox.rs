use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "chat_channel_outbox")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub origin_id: String,
    pub channel_id: i32,
    pub sender_id: String,
    pub conversation_id: i32,
    pub origin_message_id: String,
    pub turn_run_id: Option<String>,
    pub message_kind: String,
    pub final_result_id: String,
    pub content: String,
    pub chunk_index: i32,
    pub chunk_count: i32,
    pub status: String,
    pub attempt_count: i32,
    pub last_error: Option<String>,
    pub context_token_generation: Option<i64>,
    pub created_at: DateTimeUtc,
    pub delivered_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::chat_channel_turn_origin::Entity",
        from = "Column::OriginId",
        to = "super::chat_channel_turn_origin::Column::Id"
    )]
    Origin,
    #[sea_orm(
        belongs_to = "super::chat_channel::Entity",
        from = "Column::ChannelId",
        to = "super::chat_channel::Column::Id"
    )]
    ChatChannel,
}

impl Related<super::chat_channel_turn_origin::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Origin.def()
    }
}

impl Related<super::chat_channel::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ChatChannel.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
