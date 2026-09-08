use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use sea_orm::DatabaseConnection;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use super::manager::ChatChannelManager;
use super::types::{ChannelMessageTarget, DeliveryOutcome, WeixinPushMode};
use crate::db::entities::{chat_channel_outbox, chat_channel_turn_origin};
use crate::db::service::{
    chat_channel_delivery_service as delivery_service, chat_channel_message_log_service,
    chat_channel_service,
};

/// The official iLink schema does not publish a text maximum. CodeG keeps the
/// established 2,000-character interoperability limit and reserves space for
/// the `(n/m)` marker used by multi-part results.
const WEIXIN_CHUNK_BODY_CHARS: usize = 1_900;
const MAX_REPLAY_CHUNKS_PER_INBOUND: u64 = 20;
static REPLAY_IN_FLIGHT: OnceLock<Mutex<HashSet<(i32, String)>>> = OnceLock::new();

pub async fn push_mode(db: &DatabaseConnection, channel_id: i32) -> WeixinPushMode {
    match chat_channel_service::get_by_id(db, channel_id).await {
        Ok(Some(channel)) if channel.channel_type == "weixin" => {
            WeixinPushMode::from_config_json(&channel.config_json)
        }
        Ok(_) => WeixinPushMode::Debug,
        // A config read failure must not broaden a Weixin channel into debug
        // forwarding. This may temporarily suppress a non-Weixin ack, but it
        // cannot leak Web/desktop ACP events to an external recipient.
        Err(_) => WeixinPushMode::FinalAndInteractions,
    }
}

pub fn stable_origin_message_id(metadata: &serde_json::Value) -> String {
    for key in ["message_id", "msg_id", "client_id"] {
        if let Some(value) = metadata.get(key) {
            if let Some(value) = value.as_str().filter(|value| !value.is_empty()) {
                return format!("{key}:{value}");
            }
            if let Some(value) = value.as_i64() {
                return format!("{key}:{value}");
            }
            if let Some(value) = value.as_u64() {
                return format!("{key}:{value}");
            }
        }
    }
    format!("generated:{}", uuid::Uuid::new_v4())
}

pub fn client_message_id(channel_id: i32, origin_message_id: &str) -> String {
    let digest = Sha256::digest(format!("{channel_id}:{origin_message_id}").as_bytes());
    format!("chat-{channel_id}-{}", hex_prefix(&digest, 20))
}

fn hex_prefix(bytes: &[u8], length: usize) -> String {
    bytes
        .iter()
        .take(length)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn final_result_id(origin_id: &str, turn_run_id: Option<&str>, content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(origin_id.as_bytes());
    hasher.update([0]);
    hasher.update(turn_run_id.unwrap_or_default().as_bytes());
    hasher.update([0]);
    hasher.update(content.as_bytes());
    format!("final-{}", hex_prefix(&hasher.finalize(), 20))
}

pub struct OriginRegistration<'a> {
    pub channel_id: i32,
    pub sender_id: &'a str,
    pub conversation_id: i32,
    pub connection_id: Option<&'a str>,
    pub origin_message_id: &'a str,
    pub client_message_id: &'a str,
    pub target: &'a ChannelMessageTarget,
    pub prompt_blocks: &'a [crate::acp::types::PromptInputBlock],
}

pub async fn register_origin(
    db: &DatabaseConnection,
    input: OriginRegistration<'_>,
) -> Result<chat_channel_turn_origin::Model, String> {
    delivery_service::register_origin(
        db,
        delivery_service::NewTurnOrigin {
            channel_id: input.channel_id,
            sender_id: input.sender_id.to_string(),
            conversation_id: input.conversation_id,
            connection_id: input.connection_id.map(str::to_string),
            origin_message_id: input.origin_message_id.to_string(),
            client_message_id: input.client_message_id.to_string(),
            target_json: serde_json::to_string(input.target).map_err(|error| error.to_string())?,
            prompt_json: Some(
                serde_json::to_string(input.prompt_blocks).map_err(|error| error.to_string())?,
            ),
        },
    )
    .await
    .map_err(|error| error.to_string())
}

pub async fn capture_and_deliver(
    db: &DatabaseConnection,
    manager: &ChatChannelManager,
    origin: chat_channel_turn_origin::Model,
    message_kind: &str,
    content: &str,
) -> Result<(), String> {
    if !matches!(
        message_kind,
        "final" | "blocking_question" | "permission_request" | "terminal_error" | "cancelled"
    ) {
        return Err(format!(
            "message kind '{message_kind}' is not eligible for the durable outbox"
        ));
    }
    let origin = delivery_service::refresh_turn_run(db, origin)
        .await
        .map_err(|error| error.to_string())?;
    let content = normalize_final(content);
    if content.is_empty() {
        return Ok(());
    }
    let chunks = split_weixin_text(&content);
    let result_id = final_result_id(
        &origin.id,
        origin.turn_run_id.as_deref(),
        &format!("{message_kind}\0{content}"),
    );
    let rows = delivery_service::capture_chunks(db, &origin, message_kind, &result_id, &chunks)
        .await
        .map_err(|error| error.to_string())?;
    log_delivery(
        db,
        &origin,
        "final_captured",
        "queued",
        &content,
        Some(&result_id),
        None,
    )
    .await;
    log_delivery(
        db,
        &origin,
        "final_queued",
        "pending",
        "",
        Some(&result_id),
        None,
    )
    .await;
    send_rows(db, manager, rows, false).await
}

pub async fn replay_pending(
    db: &DatabaseConnection,
    manager: &ChatChannelManager,
    channel_id: i32,
    sender_id: &str,
) -> Result<(), String> {
    let key = (channel_id, sender_id.to_string());
    let in_flight = REPLAY_IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
    {
        let mut guard = in_flight.lock().await;
        if !guard.insert(key.clone()) {
            return Ok(());
        }
    }
    let result = replay_pending_inner(db, manager, channel_id, sender_id).await;
    in_flight.lock().await.remove(&key);
    result
}

async fn replay_pending_inner(
    db: &DatabaseConnection,
    manager: &ChatChannelManager,
    channel_id: i32,
    sender_id: &str,
) -> Result<(), String> {
    let rows = delivery_service::list_pending_for_sender(
        db,
        channel_id,
        sender_id,
        MAX_REPLAY_CHUNKS_PER_INBOUND,
    )
    .await
    .map_err(|error| error.to_string())?;
    if rows.is_empty() {
        return Ok(());
    }
    send_rows(db, manager, rows, true).await
}

async fn send_rows(
    db: &DatabaseConnection,
    manager: &ChatChannelManager,
    rows: Vec<chat_channel_outbox::Model>,
    replay: bool,
) -> Result<(), String> {
    let mut completed_results = HashMap::new();
    for row in rows {
        if row.status == "delivered" {
            continue;
        }
        let Some(origin) = delivery_service::find_by_id(db, &row.origin_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            continue;
        };
        let target: ChannelMessageTarget =
            serde_json::from_str(&origin.target_json).map_err(|error| error.to_string())?;
        match manager
            .send_outbox_to_target(&target, &row.content, &row.id)
            .await
        {
            Ok(DeliveryOutcome::Delivered {
                context_generation, ..
            }) => {
                delivery_service::mark_chunk_delivered(db, row.clone(), context_generation)
                    .await
                    .map_err(|error| error.to_string())?;
                log_delivery(
                    db,
                    &origin,
                    "final_chunk_sent",
                    "sent",
                    &row.content,
                    Some(&row.final_result_id),
                    None,
                )
                .await;
                completed_results.insert(row.final_result_id.clone(), origin.clone());
            }
            Ok(DeliveryOutcome::DeferredContextExpired { context_generation }) => {
                delivery_service::mark_chunk_deferred(
                    db,
                    row.clone(),
                    context_generation,
                    "context_token_expired",
                )
                .await
                .map_err(|error| error.to_string())?;
                log_delivery(
                    db,
                    &origin,
                    "context_token_expired",
                    "pending",
                    "",
                    Some(&row.final_result_id),
                    Some("waiting for next inbound WeChat context".to_string()),
                )
                .await;
                break;
            }
            Err(error) => {
                delivery_service::mark_chunk_deferred(db, row.clone(), None, &error.to_string())
                    .await
                    .map_err(|db_error| db_error.to_string())?;
                log_delivery(
                    db,
                    &origin,
                    "final_send_failed",
                    "pending",
                    "",
                    Some(&row.final_result_id),
                    Some(error.to_string()),
                )
                .await;
                break;
            }
        }
    }
    for (result_id, origin) in completed_results {
        if delivery_service::all_chunks_delivered(db, &result_id)
            .await
            .map_err(|error| error.to_string())?
        {
            log_delivery(
                db,
                &origin,
                if replay {
                    "pending_final_replayed"
                } else {
                    "final_delivered"
                },
                "sent",
                "",
                Some(&result_id),
                None,
            )
            .await;
            tracing::info!(
                stage = if replay { "pending_final_replayed" } else { "final_delivered" },
                final_result_id = %result_id,
                "[ChatChannel][final_delivery] final result fully delivered"
            );
        }
    }
    Ok(())
}

async fn log_delivery(
    db: &DatabaseConnection,
    origin: &chat_channel_turn_origin::Model,
    message_type: &str,
    status: &str,
    content: &str,
    final_result_id: Option<&str>,
    error: Option<String>,
) {
    let _ = chat_channel_message_log_service::create_correlated_log(
        db,
        origin.channel_id,
        "outbound",
        message_type,
        content,
        status,
        error,
        Some(&origin.origin_message_id),
        origin.turn_run_id.as_deref(),
        final_result_id,
    )
    .await;
}

fn normalize_final(content: &str) -> String {
    content.replace("\r\n", "\n").trim().to_string()
}

pub fn split_weixin_text(content: &str) -> Vec<String> {
    if content.chars().count() <= WEIXIN_CHUNK_BODY_CHARS {
        return vec![content.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for unit in markdown_units(content) {
        if unit.text.chars().count() > WEIXIN_CHUNK_BODY_CHARS {
            push_current(&mut chunks, &mut current);
            if let Some(fence) = unit.fence {
                split_oversized_code(&unit.text, &fence, &mut chunks);
            } else {
                split_oversized(&unit.text, &mut chunks);
            }
            continue;
        }
        if current.chars().count() + unit.text.chars().count() > WEIXIN_CHUNK_BODY_CHARS {
            push_current(&mut chunks, &mut current);
        }
        current.push_str(&unit.text);
    }
    push_current(&mut chunks, &mut current);
    let count = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| format!("({}/{})\n{}", index + 1, count, chunk.trim()))
        .collect()
}

struct MarkdownUnit {
    text: String,
    fence: Option<String>,
}

fn markdown_units(content: &str) -> Vec<MarkdownUnit> {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut units = Vec::new();
    let mut prose = String::new();
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim_start();
        let fence = if trimmed.starts_with("```") {
            Some("```".to_string())
        } else if trimmed.starts_with("~~~") {
            Some("~~~".to_string())
        } else {
            None
        };
        if let Some(fence) = fence {
            if !prose.is_empty() {
                for paragraph in prose.split_inclusive("\n\n") {
                    units.push(MarkdownUnit {
                        text: paragraph.to_string(),
                        fence: None,
                    });
                }
                prose.clear();
            }
            let mut block = String::new();
            block.push_str(lines[index]);
            index += 1;
            while index < lines.len() {
                let line = lines[index];
                block.push_str(line);
                index += 1;
                if line.trim_start().starts_with(&fence) {
                    break;
                }
            }
            units.push(MarkdownUnit {
                text: block,
                fence: Some(fence),
            });
        } else {
            prose.push_str(lines[index]);
            index += 1;
        }
    }
    if !prose.is_empty() {
        for paragraph in prose.split_inclusive("\n\n") {
            units.push(MarkdownUnit {
                text: paragraph.to_string(),
                fence: None,
            });
        }
    }
    units
}

fn split_oversized_code(block: &str, fence: &str, chunks: &mut Vec<String>) {
    let mut lines = block.lines();
    let opener = lines.next().unwrap_or(fence);
    let mut body: Vec<&str> = lines.collect();
    if body
        .last()
        .is_some_and(|line| line.trim_start().starts_with(fence))
    {
        body.pop();
    }
    let overhead = opener.chars().count() + fence.chars().count() + 2;
    let body_limit = WEIXIN_CHUNK_BODY_CHARS.saturating_sub(overhead).max(1);
    let mut current = String::new();
    for line in body {
        let with_newline = format!("{line}\n");
        if with_newline.chars().count() > body_limit {
            if !current.is_empty() {
                chunks.push(format!("{opener}\n{}{fence}", current));
                current.clear();
            }
            let chars: Vec<char> = with_newline.chars().collect();
            for slice in chars.chunks(body_limit) {
                chunks.push(format!(
                    "{opener}\n{}{fence}",
                    slice.iter().collect::<String>()
                ));
            }
        } else {
            if current.chars().count() + with_newline.chars().count() > body_limit {
                chunks.push(format!("{opener}\n{}{fence}", current));
                current.clear();
            }
            current.push_str(&with_newline);
        }
    }
    if !current.is_empty() {
        chunks.push(format!("{opener}\n{}{fence}", current));
    }
}

fn split_oversized(paragraph: &str, chunks: &mut Vec<String>) {
    let mut current = String::new();
    for piece in paragraph.split_inclusive(['\n', '。', '！', '？', '；']) {
        if piece.chars().count() > WEIXIN_CHUNK_BODY_CHARS {
            push_current(chunks, &mut current);
            let chars: Vec<char> = piece.chars().collect();
            for slice in chars.chunks(WEIXIN_CHUNK_BODY_CHARS) {
                chunks.push(slice.iter().collect());
            }
        } else {
            if current.chars().count() + piece.chars().count() > WEIXIN_CHUNK_BODY_CHARS {
                push_current(chunks, &mut current);
            }
            current.push_str(piece);
        }
    }
    push_current(chunks, &mut current);
}

fn push_current(chunks: &mut Vec<String>, current: &mut String) {
    if !current.trim().is_empty() {
        chunks.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use tokio::sync::Mutex as TokioMutex;

    use super::*;
    use crate::chat_channel::error::ChatChannelError;
    use crate::chat_channel::traits::ChatChannelBackend;
    use crate::chat_channel::types::{
        ChannelConnectionStatus, ChannelType, IncomingCommand, RichMessage, SentMessageId,
    };
    use crate::db::entities::chat_channel_outbox;
    use crate::db::service::chat_channel_service;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::models::agent::AgentType;

    #[derive(Clone, Default)]
    struct DeferredBackend {
        accepting: Arc<AtomicBool>,
        sent_keys: Arc<TokioMutex<Vec<String>>>,
    }

    #[async_trait]
    impl ChatChannelBackend for DeferredBackend {
        fn channel_type(&self) -> ChannelType {
            ChannelType::Weixin
        }

        async fn start(
            &self,
            _command_tx: tokio::sync::mpsc::Sender<IncomingCommand>,
        ) -> Result<(), ChatChannelError> {
            Ok(())
        }

        async fn stop(&self) -> Result<(), ChatChannelError> {
            Ok(())
        }

        async fn status(&self) -> ChannelConnectionStatus {
            ChannelConnectionStatus::Connected
        }

        async fn send_message(&self, _text: &str) -> Result<SentMessageId, ChatChannelError> {
            Ok(SentMessageId("ordinary".into()))
        }

        async fn send_outbox_message(
            &self,
            _text: &str,
            idempotency_key: &str,
        ) -> Result<DeliveryOutcome, ChatChannelError> {
            self.sent_keys
                .lock()
                .await
                .push(idempotency_key.to_string());
            if self.accepting.load(Ordering::SeqCst) {
                // Keep the send in flight long enough for the concurrency test
                // to prove that a second replay does not submit the same row.
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                Ok(DeliveryOutcome::Delivered {
                    message_id: SentMessageId(idempotency_key.to_string()),
                    context_generation: Some(2),
                })
            } else {
                Ok(DeliveryOutcome::DeferredContextExpired {
                    context_generation: Some(1),
                })
            }
        }

        async fn send_rich_message(
            &self,
            _message: &RichMessage,
        ) -> Result<SentMessageId, ChatChannelError> {
            Ok(SentMessageId("rich".into()))
        }

        async fn test_connection(&self) -> Result<(), ChatChannelError> {
            Ok(())
        }
    }

    async fn delivery_fixture() -> (
        crate::db::AppDatabase,
        ChatChannelManager,
        DeferredBackend,
        chat_channel_turn_origin::Model,
    ) {
        let db = fresh_in_memory_db().await;
        let channel = chat_channel_service::create(
            &db.conn,
            "Weixin".into(),
            "weixin".into(),
            r#"{"push_mode":"final_and_interactions"}"#.into(),
            true,
            false,
            None,
        )
        .await
        .unwrap();
        let folder_id = seed_folder(&db, "/tmp/codeg-final-delivery").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let target = ChannelMessageTarget {
            channel_id: channel.id,
            chat_id: None,
            thread_key: None,
            thread_kind: None,
            provider_payload: Some(serde_json::json!({"weixin_sender_id":"wx-user"})),
        };
        let origin = register_origin(
            &db.conn,
            OriginRegistration {
                channel_id: channel.id,
                sender_id: "wx-user",
                conversation_id,
                connection_id: Some("conn"),
                origin_message_id: "message_id:1",
                client_message_id: "chat-client-1",
                target: &target,
                prompt_blocks: &[],
            },
        )
        .await
        .unwrap();
        let manager = ChatChannelManager::new();
        let backend = DeferredBackend::default();
        manager
            .add_channel(
                channel.id,
                "Weixin".into(),
                ChannelType::Weixin,
                Box::new(backend.clone()),
            )
            .await
            .unwrap();
        (db, manager, backend, origin)
    }

    #[test]
    fn short_final_is_not_split() {
        assert_eq!(split_weixin_text("完成。"), vec!["完成。"]);
    }

    #[test]
    fn long_final_prefers_paragraph_boundaries_and_labels_chunks() {
        let text = format!("{}\n\n{}", "甲".repeat(1_500), "乙".repeat(1_500));
        let chunks = split_weixin_text(&text);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].starts_with("(1/2)\n"));
        assert!(chunks[1].starts_with("(2/2)\n"));
        assert!(chunks[0].contains(&"甲".repeat(100)));
        assert!(chunks[1].contains(&"乙".repeat(100)));
    }

    #[test]
    fn final_id_is_stable_for_retry() {
        assert_eq!(
            final_result_id("origin", Some("turn"), "answer"),
            final_result_id("origin", Some("turn"), "answer")
        );
    }

    #[test]
    fn oversized_code_blocks_are_closed_and_reopened_per_chunk() {
        let text = format!("```text\n{}\n```", "line of code\n".repeat(300));
        let chunks = split_weixin_text(&text);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| chunk.contains("```text\n")));
        assert!(chunks.iter().all(|chunk| chunk.trim_end().ends_with("```")));
    }

    #[tokio::test]
    async fn expired_final_replays_once_when_a_new_context_arrives() {
        let (db, manager, backend, origin) = delivery_fixture().await;
        capture_and_deliver(&db.conn, &manager, origin.clone(), "final", "最终回答")
            .await
            .unwrap();
        assert_eq!(backend.sent_keys.lock().await.len(), 1);
        let pending =
            delivery_service::list_pending_for_sender(&db.conn, origin.channel_id, "wx-user", 20)
                .await
                .unwrap();
        assert_eq!(pending.len(), 1);

        backend.accepting.store(true, Ordering::SeqCst);
        let (first, second) = tokio::join!(
            replay_pending(&db.conn, &manager, origin.channel_id, "wx-user"),
            replay_pending(&db.conn, &manager, origin.channel_id, "wx-user")
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(
            backend.sent_keys.lock().await.len(),
            2,
            "one expired attempt plus exactly one replay"
        );
        assert!(delivery_service::list_pending_for_sender(
            &db.conn,
            origin.channel_id,
            "wx-user",
            20,
        )
        .await
        .unwrap()
        .is_empty());
    }

    #[tokio::test]
    async fn process_messages_cannot_enter_the_durable_outbox() {
        let (db, manager, _backend, origin) = delivery_fixture().await;
        let error = capture_and_deliver(&db.conn, &manager, origin, "tool", "Bash")
            .await
            .unwrap_err();
        assert!(error.contains("not eligible"));
        assert_eq!(
            chat_channel_outbox::Entity::find()
                .filter(chat_channel_outbox::Column::MessageKind.eq("tool"))
                .all(&db.conn)
                .await
                .unwrap()
                .len(),
            0
        );
    }
}
