use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sea_orm::DatabaseConnection;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use super::command_handlers;
use super::i18n::{self, Lang};
use super::manager::ChatChannelManager;
use super::session_bridge::SessionBridge;
use super::session_commands;
use super::types::{
    sender_log_key, ChannelMessageTarget, IncomingCommand, InteractiveMessage, RichMessage,
    WeixinPushMode,
};
use crate::acp::manager::ConnectionManager;
use crate::db::service::{app_metadata_service, chat_channel_message_log_service};
use crate::web::event_bridge::EventEmitter;

const COMMAND_PREFIX_KEY: &str = "chat_command_prefix";
const DEFAULT_COMMAND_PREFIX: &str = "/";
const MESSAGE_LANGUAGE_KEY: &str = "chat_message_language";
/// How often to refresh cached config from DB.
const CONFIG_CACHE_TTL_SECS: u64 = 30;

struct CommandConfigCache {
    prefix: String,
    lang: Lang,
    last_refresh: Instant,
}

impl CommandConfigCache {
    fn new() -> Self {
        Self {
            prefix: DEFAULT_COMMAND_PREFIX.to_string(),
            lang: Lang::default(),
            // Force refresh on first use
            last_refresh: Instant::now() - Duration::from_secs(CONFIG_CACHE_TTL_SECS + 1),
        }
    }

    async fn refresh_if_needed(&mut self, db: &DatabaseConnection) {
        if self.last_refresh.elapsed() < Duration::from_secs(CONFIG_CACHE_TTL_SECS) {
            return;
        }

        if let Ok(Some(val)) = app_metadata_service::get_value(db, COMMAND_PREFIX_KEY).await {
            self.prefix = val;
        }
        if let Ok(Some(val)) = app_metadata_service::get_value(db, MESSAGE_LANGUAGE_KEY).await {
            self.lang = Lang::from_str_lossy(&val);
        }

        self.last_refresh = Instant::now();
    }
}

pub fn spawn_command_dispatcher(
    mut command_rx: mpsc::Receiver<IncomingCommand>,
    manager: ChatChannelManager,
    db_conn: DatabaseConnection,
    data_dir: PathBuf,
    conn_mgr: ConnectionManager,
    emitter: EventEmitter,
    bridge: Arc<Mutex<SessionBridge>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut config = CommandConfigCache::new();

        while let Some(cmd) = command_rx.recv().await {
            let text = cmd.command_text.trim();
            let origin_message_id = super::final_delivery::stable_origin_message_id(&cmd.metadata);
            let push_mode = super::final_delivery::push_mode(&db_conn, cmd.channel_id).await;
            let is_final_weixin_mode = push_mode != WeixinPushMode::Debug;
            tracing::info!(
                channel_id = cmd.channel_id,
                sender_key = %sender_log_key(&cmd.sender_id),
                message_chars = text.chars().count(),
                "[ChatChannel] received command"
            );

            // Log inbound command
            let _ = chat_channel_message_log_service::create_correlated_log(
                &db_conn,
                cmd.channel_id,
                "inbound",
                "inbound_user_message",
                text,
                "sent",
                None,
                Some(&origin_message_id),
                None,
                None,
            )
            .await;

            if is_final_weixin_mode {
                let replay_db = db_conn.clone();
                let replay_manager = manager.clone_ref();
                let replay_sender = cmd.sender_id.clone();
                let replay_channel_id = cmd.channel_id;
                tokio::spawn(async move {
                    if let Err(error) = super::final_delivery::replay_pending(
                        &replay_db,
                        &replay_manager,
                        replay_channel_id,
                        &replay_sender,
                    )
                    .await
                    {
                        tracing::warn!(
                            stage = "pending_final_replay_failed",
                            channel_id = replay_channel_id,
                            sender_key = %sender_log_key(&replay_sender),
                            error,
                            "[ChatChannel][final_delivery] pending final replay failed"
                        );
                    }
                });
            }

            config.refresh_if_needed(&db_conn).await;

            let response = dispatch_command_with_origin(
                text,
                &config.prefix,
                &db_conn,
                &manager,
                &conn_mgr,
                &emitter,
                &bridge,
                &data_dir,
                cmd.channel_id,
                &cmd.sender_id,
                &cmd.target,
                cmd.callback_data.as_deref(),
                config.lang,
                Some(&origin_message_id),
                push_mode,
            )
            .await;

            if response.message.is_none()
                && response.extra_messages.is_empty()
                && response.post_action.is_none()
            {
                tracing::debug!("[ChatChannel] dispatch result: no response");
                continue;
            };

            let mut messages = Vec::new();
            if let Some(message) = response.message {
                messages.push((message, response.target));
            }
            messages.extend(response.extra_messages);

            for (message, target) in messages {
                if is_final_weixin_mode && is_session_ack_message(&message, config.lang) {
                    tracing::debug!(
                        stage = "stale_progress_dropped",
                        channel_id = cmd.channel_id,
                        "[ChatChannel][final_delivery] delivery acknowledgement suppressed"
                    );
                    continue;
                }
                send_dispatch_message(&db_conn, &manager, cmd.channel_id, text, message, target)
                    .await;
            }

            if let Some(action) = response.post_action {
                if let Some((message, target)) = session_commands::handle_post_action(
                    action,
                    &db_conn,
                    &conn_mgr,
                    &bridge,
                    Some(&origin_message_id),
                    push_mode,
                )
                .await
                {
                    if is_final_weixin_mode && message.body == i18n::message_sent(config.lang) {
                        continue;
                    }
                    send_dispatch_message(
                        &db_conn,
                        &manager,
                        cmd.channel_id,
                        text,
                        DispatchMessage::Rich(message),
                        target,
                    )
                    .await;
                }
            }
        }
    })
}

async fn send_dispatch_message(
    db: &DatabaseConnection,
    manager: &ChatChannelManager,
    channel_id: i32,
    command_text: &str,
    message: DispatchMessage,
    target: ChannelMessageTarget,
) {
    tracing::info!(
        "[ChatChannel] dispatch result: title={:?}, body_len={}",
        message.title(),
        message.body_len()
    );

    let send_result = match &message {
        DispatchMessage::Rich(message) => manager.send_to_target(&target, message).await,
        DispatchMessage::Interactive(message) => {
            manager.send_interactive_to_target(&target, message).await
        }
    };
    let (status, error_detail) = match &send_result {
        Ok(_) => ("sent", None),
        Err(e) => {
            tracing::error!(
                channel_id,
                command_chars = command_text.chars().count(),
                error = %e,
                "[ChatChannel] failed to send response"
            );
            ("failed", Some(e.to_string()))
        }
    };

    let _ = chat_channel_message_log_service::create_log(
        db,
        channel_id,
        "outbound",
        "command_response",
        &message.to_plain_text(),
        status,
        error_detail,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
async fn dispatch_command(
    text: &str,
    prefix: &str,
    db: &DatabaseConnection,
    manager: &ChatChannelManager,
    conn_mgr: &ConnectionManager,
    emitter: &EventEmitter,
    bridge: &Arc<Mutex<SessionBridge>>,
    data_dir: &Path,
    channel_id: i32,
    sender_id: &str,
    target: &ChannelMessageTarget,
    callback_data: Option<&str>,
    lang: Lang,
) -> DispatchResponse {
    dispatch_command_with_origin(
        text,
        prefix,
        db,
        manager,
        conn_mgr,
        emitter,
        bridge,
        data_dir,
        channel_id,
        sender_id,
        target,
        callback_data,
        lang,
        None,
        WeixinPushMode::Debug,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_command_with_origin(
    text: &str,
    prefix: &str,
    db: &DatabaseConnection,
    manager: &ChatChannelManager,
    conn_mgr: &ConnectionManager,
    emitter: &EventEmitter,
    bridge: &Arc<Mutex<SessionBridge>>,
    data_dir: &Path,
    channel_id: i32,
    sender_id: &str,
    target: &ChannelMessageTarget,
    callback_data: Option<&str>,
    lang: Lang,
    origin_message_id: Option<&str>,
    push_mode: WeixinPushMode,
) -> DispatchResponse {
    if let Some(data) = callback_data {
        return DispatchResponse::current(
            session_commands::handle_callback(db, data, channel_id, sender_id, lang, prefix).await,
            target,
        );
    }

    // Strip prefix; if text doesn't start with it, try as follow-up
    let without_prefix = match text.strip_prefix(prefix) {
        Some(rest) => rest,
        None => {
            if target.is_telegram_general_topic() {
                return DispatchResponse::none(target);
            }

            if target.is_telegram_forum_topic() {
                return DispatchResponse::current(
                    session_commands::handle_followup(session_commands::FollowupRequest {
                        db,
                        text,
                        channel_id,
                        sender_id,
                        target,
                        conn_mgr,
                        emitter,
                        bridge,
                        data_dir,
                        lang,
                        prefix,
                        origin_message_id,
                        push_mode,
                    })
                    .await,
                    target,
                );
            }

            // The bridge is intentionally in-memory. Always let follow-up
            // routing inspect the durable sender context so the first message
            // after a restart can restore its previous conversation instead
            // of falling through to Help.
            tracing::info!(
                stage = "ordinary_message_followup_dispatched",
                channel_id,
                sender_key = %sender_log_key(sender_id),
                "[ChatChannel][lazy_restore] ordinary message entered follow-up routing"
            );
            return DispatchResponse::current(
                session_commands::handle_followup(session_commands::FollowupRequest {
                    db,
                    text,
                    channel_id,
                    sender_id,
                    target,
                    conn_mgr,
                    emitter,
                    bridge,
                    data_dir,
                    lang,
                    prefix,
                    origin_message_id,
                    push_mode,
                })
                .await,
                target,
            );
        }
    };

    let parts: Vec<&str> = without_prefix.splitn(2, ' ').collect();
    let command = parts[0].to_lowercase();
    let args = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match command.as_str() {
        // Existing commands
        "search" => {
            if args.is_empty() {
                DispatchResponse::current(
                    RichMessage::info(i18n::search_usage(lang, prefix))
                        .with_title(i18n::invalid_args_title(lang)),
                    target,
                )
            } else {
                DispatchResponse::current(
                    command_handlers::handle_search(db, args, lang).await,
                    target,
                )
            }
        }
        "today" => {
            DispatchResponse::current(command_handlers::handle_today(db, lang).await, target)
        }
        "status" => {
            DispatchResponse::current(command_handlers::handle_status(manager, lang).await, target)
        }
        "help" | "start" => {
            DispatchResponse::current(command_handlers::handle_help(prefix, lang), target)
        }

        // Session commands
        "folder" => {
            if args.is_empty() {
                DispatchResponse::from_session_message(
                    session_commands::handle_folder_picker(db, channel_id, sender_id, lang, prefix)
                        .await,
                    target,
                )
            } else {
                DispatchResponse::current(
                    session_commands::handle_folder(db, args, channel_id, sender_id, lang, prefix)
                        .await,
                    target,
                )
            }
        }
        "agent" => {
            if args.is_empty() {
                DispatchResponse::from_session_message(
                    session_commands::handle_agent_picker(db, channel_id, sender_id, lang, prefix)
                        .await,
                    target,
                )
            } else {
                DispatchResponse::current(
                    session_commands::handle_agent(db, args, channel_id, sender_id, lang, prefix)
                        .await,
                    target,
                )
            }
        }
        "task" | "do" => {
            let result = session_commands::handle_task(
                db, args, channel_id, sender_id, target, manager, conn_mgr, emitter, bridge, lang,
                prefix, data_dir,
            )
            .await;
            DispatchResponse {
                message: Some(DispatchMessage::Rich(result.message)),
                target: result.response_target,
                extra_messages: result
                    .extra_responses
                    .into_iter()
                    .map(|(message, target)| (DispatchMessage::Rich(message), target))
                    .collect(),
                post_action: result.post_action,
            }
        }
        "sessions" => DispatchResponse::current(
            session_commands::handle_sessions(db, channel_id, sender_id, target, lang, prefix)
                .await,
            target,
        ),
        "resume" => DispatchResponse::current(
            session_commands::handle_resume(
                db, args, channel_id, sender_id, target, manager, conn_mgr, emitter, bridge, lang,
                prefix, data_dir,
            )
            .await,
            target,
        ),
        "cancel" => DispatchResponse::current(
            session_commands::handle_cancel(
                db, channel_id, sender_id, target, conn_mgr, bridge, lang,
            )
            .await,
            target,
        ),
        "approve" => {
            let always = args.eq_ignore_ascii_case("always");
            DispatchResponse::current(
                session_commands::handle_permission_response(
                    true, always, db, channel_id, sender_id, target, conn_mgr, bridge, lang,
                )
                .await,
                target,
            )
        }
        "deny" => DispatchResponse::current(
            session_commands::handle_permission_response(
                false, false, db, channel_id, sender_id, target, conn_mgr, bridge, lang,
            )
            .await,
            target,
        ),

        _ => DispatchResponse::current(
            RichMessage::info(i18n::unknown_command(lang, prefix, &command))
                .with_title(i18n::unknown_command_title(lang)),
            target,
        ),
    }
}

struct DispatchResponse {
    message: Option<DispatchMessage>,
    target: ChannelMessageTarget,
    extra_messages: Vec<(DispatchMessage, ChannelMessageTarget)>,
    post_action: Option<session_commands::CommandPostAction>,
}

impl DispatchResponse {
    fn current(message: RichMessage, target: &ChannelMessageTarget) -> Self {
        Self {
            message: Some(DispatchMessage::Rich(message)),
            target: target.clone(),
            extra_messages: Vec::new(),
            post_action: None,
        }
    }

    fn from_session_message(
        message: session_commands::SessionCommandMessage,
        target: &ChannelMessageTarget,
    ) -> Self {
        Self {
            message: Some(match message {
                session_commands::SessionCommandMessage::Rich(message) => {
                    DispatchMessage::Rich(message)
                }
                session_commands::SessionCommandMessage::Interactive(message) => {
                    DispatchMessage::Interactive(message)
                }
            }),
            target: target.clone(),
            extra_messages: Vec::new(),
            post_action: None,
        }
    }

    fn none(target: &ChannelMessageTarget) -> Self {
        Self {
            message: None,
            target: target.clone(),
            extra_messages: Vec::new(),
            post_action: None,
        }
    }
}

enum DispatchMessage {
    Rich(RichMessage),
    Interactive(InteractiveMessage),
}

impl DispatchMessage {
    fn title(&self) -> Option<&String> {
        match self {
            Self::Rich(message) => message.title.as_ref(),
            Self::Interactive(message) => message.base.title.as_ref(),
        }
    }

    fn body_len(&self) -> usize {
        match self {
            Self::Rich(message) => message.body.len(),
            Self::Interactive(message) => message.base.body.len(),
        }
    }

    fn to_plain_text(&self) -> String {
        match self {
            Self::Rich(message) => message.to_plain_text(),
            Self::Interactive(message) => message.to_rich_fallback().to_plain_text(),
        }
    }
}

fn is_session_ack_message(message: &DispatchMessage, lang: Lang) -> bool {
    let (title, body) = match message {
        DispatchMessage::Rich(message) => (message.title.as_deref(), message.body.as_str()),
        DispatchMessage::Interactive(message) => {
            (message.base.title.as_deref(), message.base.body.as_str())
        }
    };
    body == i18n::message_sent(lang)
        || body == i18n::task_deferred_busy(lang)
        || title == Some(i18n::task_started_title(lang))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::connection::ConnectionCommand;
    use crate::acp::types::{AcpEvent, EventEnvelope, PromptInputBlock};
    use crate::chat_channel::error::ChatChannelError;
    use crate::chat_channel::traits::ChatChannelBackend;
    use crate::chat_channel::types::{ChannelConnectionStatus, ChannelType, SentMessageId};
    use crate::db::service::{chat_channel_service, conversation_service, sender_context_service};
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::models::AgentType;
    use async_trait::async_trait;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct Recorder {
        messages: Arc<Mutex<Vec<String>>>,
    }

    struct RecordingWeixinBackend {
        recorder: Recorder,
    }

    struct FailingWeixinBackend;

    #[async_trait]
    impl ChatChannelBackend for RecordingWeixinBackend {
        fn channel_type(&self) -> ChannelType {
            ChannelType::Weixin
        }

        async fn start(
            &self,
            _command_tx: mpsc::Sender<IncomingCommand>,
        ) -> Result<(), ChatChannelError> {
            Ok(())
        }

        async fn stop(&self) -> Result<(), ChatChannelError> {
            Ok(())
        }

        async fn status(&self) -> ChannelConnectionStatus {
            ChannelConnectionStatus::Connected
        }

        async fn send_message(&self, text: &str) -> Result<SentMessageId, ChatChannelError> {
            self.recorder.messages.lock().await.push(text.to_string());
            Ok(SentMessageId("weixin-test-message".into()))
        }

        async fn send_rich_message(
            &self,
            message: &RichMessage,
        ) -> Result<SentMessageId, ChatChannelError> {
            self.recorder
                .messages
                .lock()
                .await
                .push(message.body.clone());
            Ok(SentMessageId("weixin-test-message".into()))
        }

        async fn test_connection(&self) -> Result<(), ChatChannelError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ChatChannelBackend for FailingWeixinBackend {
        fn channel_type(&self) -> ChannelType {
            ChannelType::Weixin
        }

        async fn start(
            &self,
            _command_tx: mpsc::Sender<IncomingCommand>,
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
            Err(ChatChannelError::SendFailed(
                "temporary network error".into(),
            ))
        }

        async fn send_rich_message(
            &self,
            _message: &RichMessage,
        ) -> Result<SentMessageId, ChatChannelError> {
            Err(ChatChannelError::SendFailed(
                "temporary network error".into(),
            ))
        }

        async fn test_connection(&self) -> Result<(), ChatChannelError> {
            Ok(())
        }
    }

    async fn seed_chat_channel(db: &crate::db::AppDatabase) -> i32 {
        chat_channel_service::create(
            &db.conn,
            "Telegram test".to_string(),
            "telegram".to_string(),
            serde_json::json!({ "chat_id": "-100123", "topic_mode": true }).to_string(),
            true,
            false,
            None,
        )
        .await
        .expect("seed chat channel")
        .id
    }

    async fn seed_weixin_channel(db: &crate::db::AppDatabase) -> i32 {
        chat_channel_service::create(
            &db.conn,
            "Weixin lazy restore test".to_string(),
            "weixin".to_string(),
            serde_json::json!({ "base_url": "https://example.invalid" }).to_string(),
            true,
            false,
            None,
        )
        .await
        .expect("seed Weixin chat channel")
        .id
    }

    #[tokio::test]
    async fn callback_data_dispatches_without_command_prefix() {
        let db = fresh_in_memory_db().await;
        let channel_id = seed_chat_channel(&db).await;
        let folder_id = seed_folder(&db, "/tmp/codeg-dispatch-callback").await;
        let target = ChannelMessageTarget::telegram_general(channel_id, "-100123");
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));

        let response = dispatch_command(
            "cfg:folder:ignored-by-callback-data",
            "/",
            &db.conn,
            &ChatChannelManager::new(),
            &ConnectionManager::new(),
            &EventEmitter::Noop,
            &bridge,
            std::path::Path::new("/tmp/codeg-dispatch-data"),
            channel_id,
            "sender-1",
            &target,
            Some(&format!("cfg:folder:{folder_id}")),
            Lang::En,
        )
        .await;
        let ctx = sender_context_service::get_or_create(&db.conn, channel_id, "sender-1")
            .await
            .expect("context");

        assert!(matches!(response.message, Some(DispatchMessage::Rich(_))));
        assert_eq!(ctx.current_folder_id, Some(folder_id));
    }

    #[tokio::test]
    async fn general_topic_plain_text_returns_no_response() {
        let db = fresh_in_memory_db().await;
        let channel_id = seed_chat_channel(&db).await;
        let target = ChannelMessageTarget::telegram_general(channel_id, "-100123");
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));

        let response = dispatch_command(
            "hello group",
            "/",
            &db.conn,
            &ChatChannelManager::new(),
            &ConnectionManager::new(),
            &EventEmitter::Noop,
            &bridge,
            std::path::Path::new("/tmp/codeg-dispatch-data"),
            channel_id,
            "sender-1",
            &target,
            None,
            Lang::En,
        )
        .await;

        assert!(response.message.is_none());
        assert_eq!(response.target, target);
    }

    #[tokio::test]
    async fn ordinary_plain_text_uses_followup_router_instead_of_help_fallback() {
        let db = fresh_in_memory_db().await;
        let channel_id = seed_chat_channel(&db).await;
        let target = ChannelMessageTarget::channel(channel_id);
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));

        let response = dispatch_command(
            "hello",
            "/",
            &db.conn,
            &ChatChannelManager::new(),
            &ConnectionManager::new(),
            &EventEmitter::Noop,
            &bridge,
            std::path::Path::new("/tmp/codeg-dispatch-data"),
            channel_id,
            "sender-1",
            &target,
            None,
            Lang::En,
        )
        .await;
        let Some(DispatchMessage::Rich(message)) = response.message else {
            panic!("ordinary text should receive follow-up status")
        };
        assert!(message.body.contains("/task"));
        assert_ne!(message.title.as_deref(), Some("Codeg Bot Help"));
        assert!(
            sender_context_service::find(&db.conn, channel_id, "sender-1")
                .await
                .unwrap()
                .is_none(),
            "an unbound sender must not get a manufactured empty context row"
        );
    }

    #[tokio::test]
    async fn unbound_weixin_sender_applies_channel_default_and_forwards_once() {
        let db = fresh_in_memory_db().await;
        let channel_id = seed_weixin_channel(&db).await;
        let folder_path = "/tmp/codeg-weixin-default-restore";
        let folder_id = seed_folder(&db, folder_path).await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::OpenCode).await;
        conversation_service::update_external_id(
            &db.conn,
            conversation_id,
            "weixin-default-session".into(),
        )
        .await
        .unwrap();
        chat_channel_service::update(
            &db.conn,
            channel_id,
            None,
            None,
            Some(
                serde_json::json!({
                    "base_url": "https://example.invalid",
                    "default_folder_id": folder_id,
                    "default_agent_type": "open_code",
                    "default_conversation_id": conversation_id,
                })
                .to_string(),
            ),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let conn_mgr = ConnectionManager::new();
        let mut agent_commands = conn_mgr
            .insert_test_connection_live(
                "weixin-default-connection",
                AgentType::OpenCode,
                Some(PathBuf::from(folder_path)),
                EventEmitter::Noop,
            )
            .await;
        {
            let state = conn_mgr
                .get_state("weixin-default-connection")
                .await
                .unwrap();
            let mut state = state.write().await;
            state.external_id = Some("weixin-default-session".into());
            state.selectors_ready = true;
        }
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));
        let response = dispatch_command(
            "你好",
            "/",
            &db.conn,
            &ChatChannelManager::new(),
            &conn_mgr,
            &EventEmitter::Noop,
            &bridge,
            std::path::Path::new("/tmp/codeg-dispatch-data"),
            channel_id,
            "new-weixin-sender",
            &ChannelMessageTarget::channel(channel_id),
            None,
            Lang::ZhCn,
        )
        .await;

        let command = tokio::time::timeout(Duration::from_secs(2), agent_commands.recv())
            .await
            .expect("default restore timed out")
            .expect("forwarded prompt");
        let ConnectionCommand::Prompt { blocks, .. } = command else {
            panic!("expected prompt command")
        };
        assert!(matches!(
            blocks.as_slice(),
            [PromptInputBlock::Text { text }] if text == "你好"
        ));
        assert!(agent_commands.try_recv().is_err());
        let context = sender_context_service::find(&db.conn, channel_id, "new-weixin-sender")
            .await
            .unwrap()
            .expect("default binding persisted");
        assert_eq!(context.current_folder_id, Some(folder_id));
        assert_eq!(context.current_agent_type.as_deref(), Some("open_code"));
        assert_eq!(context.current_conversation_id, Some(conversation_id));
        assert_eq!(
            context.current_connection_id.as_deref(),
            Some("weixin-default-connection")
        );
        let Some(DispatchMessage::Rich(message)) = response.message else {
            panic!("default restore should acknowledge the forwarded message")
        };
        assert_eq!(message.body, i18n::message_sent(Lang::ZhCn));
    }

    #[tokio::test]
    async fn weixin_reply_failure_does_not_clear_sender_binding() {
        let db = fresh_in_memory_db().await;
        let channel_id = seed_weixin_channel(&db).await;
        let folder_id = seed_folder(&db, "/tmp/codeg-weixin-send-failure").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        sender_context_service::update_session(
            &db.conn,
            channel_id,
            "wx-user",
            Some(conversation_id),
            Some("still-valid".into()),
        )
        .await
        .unwrap();
        let manager = ChatChannelManager::new();
        manager
            .add_channel(
                channel_id,
                "Failing Weixin".into(),
                ChannelType::Weixin,
                Box::new(FailingWeixinBackend),
            )
            .await
            .unwrap();

        send_dispatch_message(
            &db.conn,
            &manager,
            channel_id,
            "hello",
            DispatchMessage::Rich(RichMessage::info("reply")),
            ChannelMessageTarget::channel(channel_id),
        )
        .await;

        let context = sender_context_service::find(&db.conn, channel_id, "wx-user")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(context.current_conversation_id, Some(conversation_id));
        assert_eq!(
            context.current_connection_id.as_deref(),
            Some("still-valid")
        );
    }

    #[tokio::test]
    async fn weixin_restart_lazily_restores_and_forwards_original_message_once() {
        let db = fresh_in_memory_db().await;
        let channel_id = seed_weixin_channel(&db).await;
        let folder_path = "/tmp/codeg-weixin-lazy-restore";
        let folder_id = seed_folder(&db, folder_path).await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::OpenCode).await;
        conversation_service::update_external_id(
            &db.conn,
            conversation_id,
            "weixin-persisted-session".into(),
        )
        .await
        .expect("persist external session");
        sender_context_service::update_session(
            &db.conn,
            channel_id,
            "wx-user",
            Some(conversation_id),
            Some("stale-before-restart".into()),
        )
        .await
        .expect("persist sender conversation binding");

        let conn_mgr = ConnectionManager::new();
        let mut agent_commands = conn_mgr
            .insert_test_connection_live(
                "weixin-restored-connection",
                AgentType::OpenCode,
                Some(PathBuf::from(folder_path)),
                EventEmitter::Noop,
            )
            .await;
        let state = conn_mgr
            .get_state("weixin-restored-connection")
            .await
            .expect("test ACP state");
        {
            let mut state = state.write().await;
            state.external_id = Some("weixin-persisted-session".into());
            state.selectors_ready = true;
        }

        // A fresh bridge models codeg-server restart/reconnect. The durable DB
        // route above is the only place that still knows the conversation.
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));
        let target = ChannelMessageTarget::channel(channel_id);
        let manager = ChatChannelManager::new();
        let recorder = Recorder::default();
        manager
            .add_channel(
                channel_id,
                "Weixin test".into(),
                ChannelType::Weixin,
                Box::new(RecordingWeixinBackend {
                    recorder: recorder.clone(),
                }),
            )
            .await
            .expect("register recording Weixin backend");

        let (command_tx, command_rx) = mpsc::channel(4);
        let dispatcher = spawn_command_dispatcher(
            command_rx,
            manager.clone_ref(),
            db.conn.clone(),
            PathBuf::from("/tmp/codeg-dispatch-data"),
            conn_mgr.clone_ref(),
            EventEmitter::Noop,
            bridge.clone(),
        );
        command_tx
            .send(IncomingCommand {
                channel_id,
                sender_id: "wx-user".into(),
                command_text: "你好".into(),
                callback_data: None,
                target: target.clone(),
                metadata: serde_json::json!({ "provider": "weixin" }),
            })
            .await
            .expect("submit ordinary Weixin text to dispatcher");

        let command = tokio::time::timeout(Duration::from_secs(2), agent_commands.recv())
            .await
            .expect("dispatcher timed out")
            .expect("forwarded ACP prompt");
        let ConnectionCommand::Prompt { blocks, .. } = command else {
            panic!("expected prompt command")
        };
        assert!(matches!(
            blocks.as_slice(),
            [PromptInputBlock::Text { text }] if text == "你好"
        ));
        assert!(
            agent_commands.try_recv().is_err(),
            "the original message must be submitted exactly once"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            recorder.messages.lock().await.is_empty(),
            "final-result mode must not send a Message sent acknowledgement"
        );

        let active = bridge.lock().await;
        let session = active
            .find_by_sender(channel_id, "wx-user")
            .expect("ActiveSession registered after restore");
        assert_eq!(session.conversation_id, conversation_id);
        assert_eq!(session.connection_id, "weixin-restored-connection");
        assert!(session.forward_events);
        drop(active);
        let persisted = sender_context_service::find(&db.conn, channel_id, "wx-user")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.current_conversation_id, Some(conversation_id));
        assert_eq!(
            persisted.current_connection_id.as_deref(),
            Some("weixin-restored-connection")
        );

        // Model output is still scoped to the Weixin-owned turn and reaches
        // the channel backend after the lazy restore.
        super::super::session_event_subscriber::handle_acp_envelope(
            &EventEnvelope {
                seq: 1,
                connection_id: "weixin-restored-connection".into(),
                payload: AcpEvent::ContentDelta {
                    text: "你好，我已经恢复了原会话。".into(),
                    parent_tool_use_id: None,
                },
            },
            &bridge,
            &manager,
            &conn_mgr,
            &db.conn,
        )
        .await;
        {
            let state = conn_mgr
                .get_state("weixin-restored-connection")
                .await
                .expect("restored ACP state");
            state.write().await.last_assistant_text =
                Some("你好，我已经恢复了原会话。".into());
        }
        super::super::session_event_subscriber::handle_acp_envelope(
            &EventEnvelope {
                seq: 2,
                connection_id: "weixin-restored-connection".into(),
                payload: AcpEvent::TurnComplete {
                    session_id: "weixin-persisted-session".into(),
                    stop_reason: "end_turn".into(),
                    agent_type: "opencode".into(),
                },
            },
            &bridge,
            &manager,
            &conn_mgr,
            &db.conn,
        )
        .await;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !recorder.messages.lock().await.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("Weixin model reply timed out");
        let sent = recorder.messages.lock().await.clone();
        assert_eq!(sent, vec!["你好，我已经恢复了原会话。"]);
        dispatcher.abort();
    }
}
