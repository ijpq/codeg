use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sea_orm::DatabaseConnection;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::i18n::Lang;
use super::manager::ChatChannelManager;
use super::message_formatter;
use super::session_bridge::SessionBridge;
use super::types::RichMessage;
use crate::acp::internal_bus::InternalEventBus;
use crate::acp::types::{AcpEvent, EventEnvelope};
use crate::db::service::{
    app_metadata_service, chat_channel_message_log_service, chat_channel_service,
};

/// Minimum interval between pushes for the same event type per channel (debounce).
const DEBOUNCE_SECS: u64 = 5;
/// How often to refresh cached config from DB.
const CONFIG_CACHE_TTL_SECS: u64 = 30;

const MESSAGE_LANGUAGE_KEY: &str = "chat_message_language";
const EVENT_FILTER_KEY: &str = "chat_event_filter";

struct CachedChannel {
    id: i32,
    event_filter_json: Option<String>,
}

struct EventConfigCache {
    lang: Lang,
    global_filter: Option<Vec<String>>,
    enabled_channels: Vec<CachedChannel>,
    last_refresh: Instant,
}

impl EventConfigCache {
    fn new() -> Self {
        Self {
            lang: Lang::default(),
            global_filter: None,
            enabled_channels: Vec::new(),
            // Force refresh on first use
            last_refresh: Instant::now() - Duration::from_secs(CONFIG_CACHE_TTL_SECS + 1),
        }
    }

    async fn refresh_if_needed(&mut self, db: &DatabaseConnection) {
        if self.last_refresh.elapsed() < Duration::from_secs(CONFIG_CACHE_TTL_SECS) {
            return;
        }

        if let Ok(Some(val)) = app_metadata_service::get_value(db, MESSAGE_LANGUAGE_KEY).await {
            self.lang = Lang::from_str_lossy(&val);
        }

        // Parse as Option<Vec<String>> so JSON "null" → None (intentional, not accidental)
        self.global_filter = app_metadata_service::get_value(db, EVENT_FILTER_KEY)
            .await
            .ok()
            .flatten()
            .and_then(|json| {
                serde_json::from_str::<Option<Vec<String>>>(&json)
                    .ok()
                    .flatten()
            });

        if let Ok(channels) = chat_channel_service::list_enabled(db).await {
            self.enabled_channels = channels
                .into_iter()
                .map(|ch| CachedChannel {
                    id: ch.id,
                    event_filter_json: ch.event_filter_json,
                })
                .collect();
        }

        self.last_refresh = Instant::now();
    }
}

pub fn spawn_event_subscriber(
    bus: Arc<InternalEventBus>,
    manager: ChatChannelManager,
    db_conn: DatabaseConnection,
    bridge: Arc<Mutex<SessionBridge>>,
) -> JoinHandle<()> {
    // Subscribe synchronously before the spawn so the broadcast buffer
    // catches any events emitted in the gap between `start_background`
    // returning and the spawned task's first `rx.recv().await` poll.
    let mut rx = bus.subscribe();
    let metrics = Arc::clone(bus.metrics());

    tokio::spawn(async move {
        let mut last_push: HashMap<(i32, String), Instant> = HashMap::new();
        let mut config = EventConfigCache::new();

        loop {
            let envelope_arc = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("[ChatChannel] event subscriber lagged by {n} messages");
                    metrics.lagged_count.fetch_add(n, Ordering::Relaxed);
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    eprintln!("[ChatChannel] internal bus closed, stopping subscriber");
                    break;
                }
            };

            config.refresh_if_needed(&db_conn).await;

            // Prune stale debounce entries
            last_push.retain(|_, t| t.elapsed() < Duration::from_secs(DEBOUNCE_SECS * 2));

            process_envelope(
                envelope_arc.as_ref(),
                &bridge,
                &manager,
                &db_conn,
                &config,
                &mut last_push,
            )
            .await;
        }
    })
}

/// Handle a single bus envelope: map it to a chat-channel push, apply the
/// global + per-channel event filters and the per-(channel, event) debounce,
/// then fan out to the enabled channels and log the outcome.
///
/// Extracted from the subscriber loop so the filter/dedup/debounce logic is
/// unit-testable against a recording backend.
async fn process_envelope(
    envelope: &EventEnvelope,
    bridge: &Arc<Mutex<SessionBridge>>,
    manager: &ChatChannelManager,
    db_conn: &DatabaseConnection,
    config: &EventConfigCache,
    last_push: &mut HashMap<(i32, String), Instant>,
) {
    let Some((event_type, msg)) = parse_acp_event(&envelope.payload, config.lang) else {
        return;
    };

    // Global event filter first — a filtered-out event needs no bridge lock or
    // fan-out work.
    if let Some(filter) = &config.global_filter {
        if !filter.contains(&event_type) {
            return;
        }
    }

    // A permission request from a session that was started FROM a chat channel
    // is already handled interactively (with `/approve`, `/deny`) by the session
    // relay, scoped to its owning channel. Suppress the generic global push for
    // those connections so they aren't double-notified — the global event feed
    // exists for the desktop / web sessions the user isn't driving from chat.
    if event_type == "permission_request"
        && bridge.lock().await.get(&envelope.connection_id).is_some()
    {
        return;
    }

    // Permission requests bypass the per-(channel, event) debounce. That debounce
    // throttles high-frequency events like turn_complete, but a permission gate is
    // a discrete, blocking, actionable event: a second gate on the same connection
    // (sequential) or a second concurrent agent's gate within the 5s window would
    // otherwise be silently dropped — and a blocked agent emits no further event
    // to re-trigger the lost nudge. So each one is always delivered.
    let debounced = event_type != "permission_request";

    for ch in &config.enabled_channels {
        // Per-channel event filter
        if let Some(filter_json) = &ch.event_filter_json {
            if let Ok(filter) = serde_json::from_str::<Vec<String>>(filter_json) {
                if !filter.contains(&event_type) {
                    continue;
                }
            }
        }

        // Debounce: skip if the same event type was pushed to this channel
        // recently (permission_request is exempt — see above).
        let key = (ch.id, event_type.clone());
        let now = Instant::now();
        if debounced {
            if let Some(last) = last_push.get(&key) {
                if now.duration_since(*last) < Duration::from_secs(DEBOUNCE_SECS) {
                    continue;
                }
            }
        }

        // Send
        let send_result = manager.send_to_channel(ch.id, &msg).await;
        let (status, error_detail) = match &send_result {
            Ok(_) => {
                // Only update the debounce timestamp on success, and only for
                // debounced event types.
                if debounced {
                    last_push.insert(key, now);
                }
                ("sent", None)
            }
            Err(e) => ("failed", Some(e.to_string())),
        };

        let _ = chat_channel_message_log_service::create_log(
            db_conn,
            ch.id,
            "outbound",
            "event_push",
            &msg.to_plain_text(),
            status,
            error_detail,
        )
        .await;
    }
}

/// Map an ACP event into the chat-channel push tuple. Pattern-match on the
/// typed `AcpEvent` variant — Phase 5 source-of-truth replaces the prior
/// JSON `type`-string dispatch (which paid `serde_json::from_value` per
/// event for the global broadcaster path).
fn parse_acp_event(payload: &AcpEvent, lang: Lang) -> Option<(String, RichMessage)> {
    match payload {
        AcpEvent::TurnComplete {
            stop_reason,
            agent_type,
            ..
        } => {
            // Only push for end_turn, not for intermediate completions.
            if stop_reason != "end_turn" {
                return None;
            }
            Some((
                "turn_complete".to_string(),
                message_formatter::format_turn_complete(agent_type, stop_reason, lang),
            ))
        }
        AcpEvent::Error {
            message,
            agent_type,
            ..
        } => Some((
            "error".to_string(),
            message_formatter::format_agent_error(agent_type, message, lang),
        )),
        AcpEvent::PermissionRequest { tool_call, .. } => Some((
            "permission_request".to_string(),
            message_formatter::format_permission_request(tool_call, lang),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod permission_push_tests {
    //! Coverage for the global permission-request push: it fires for desktop /
    //! web sessions, is suppressed for chat-channel-bridged connections (whose
    //! interactive `/approve` flow lives in the session relay), still honours
    //! the global event filter, and doesn't regress the existing turn_complete
    //! push after the `process_envelope` extraction.
    use super::*;
    use crate::chat_channel::error::ChatChannelError;
    use crate::chat_channel::session_bridge::ActiveSession;
    use crate::chat_channel::traits::ChatChannelBackend;
    use crate::chat_channel::types::{
        ChannelConnectionStatus, ChannelType, IncomingCommand, SentMessageId,
    };
    use crate::db::test_helpers;
    use crate::models::agent::AgentType;
    use async_trait::async_trait;
    use std::collections::HashSet;
    use tokio::sync::mpsc;

    /// Channel backend that records the rendered plain text of every message,
    /// so a test can assert the exact lines pushed (or that none were).
    #[derive(Clone, Default)]
    struct Recorder {
        msgs: Arc<Mutex<Vec<String>>>,
    }
    struct RecordingBackend {
        rec: Recorder,
    }

    #[async_trait]
    impl ChatChannelBackend for RecordingBackend {
        fn channel_type(&self) -> ChannelType {
            ChannelType::Telegram
        }
        async fn start(&self, _tx: mpsc::Sender<IncomingCommand>) -> Result<(), ChatChannelError> {
            Ok(())
        }
        async fn stop(&self) -> Result<(), ChatChannelError> {
            Ok(())
        }
        async fn status(&self) -> ChannelConnectionStatus {
            ChannelConnectionStatus::Connected
        }
        async fn send_message(&self, text: &str) -> Result<SentMessageId, ChatChannelError> {
            self.rec.msgs.lock().await.push(text.to_string());
            Ok(SentMessageId("1".into()))
        }
        async fn send_rich_message(
            &self,
            message: &RichMessage,
        ) -> Result<SentMessageId, ChatChannelError> {
            self.rec.msgs.lock().await.push(message.to_plain_text());
            Ok(SentMessageId("1".into()))
        }
        async fn test_connection(&self) -> Result<(), ChatChannelError> {
            Ok(())
        }
    }

    async fn manager_with_recorder(channel_id: i32) -> (ChatChannelManager, Recorder) {
        let chat = ChatChannelManager::new();
        let rec = Recorder::default();
        chat.add_channel(
            channel_id,
            "test".into(),
            ChannelType::Telegram,
            Box::new(RecordingBackend { rec: rec.clone() }),
        )
        .await
        .unwrap();
        (chat, rec)
    }

    fn config_all_on(channel_id: i32) -> EventConfigCache {
        EventConfigCache {
            lang: Lang::En,
            global_filter: None,
            enabled_channels: vec![CachedChannel {
                id: channel_id,
                event_filter_json: None,
            }],
            last_refresh: Instant::now(),
        }
    }

    fn permission_envelope(connection_id: &str) -> EventEnvelope {
        EventEnvelope {
            seq: 1,
            connection_id: connection_id.into(),
            payload: AcpEvent::PermissionRequest {
                request_id: "req-1".into(),
                tool_call: serde_json::json!({
                    "title": "Bash",
                    "rawInput": { "command": "npm test" }
                }),
                options: vec![],
            },
        }
    }

    fn bridged_session(connection_id: &str, channel_id: i32) -> ActiveSession {
        ActiveSession {
            channel_id,
            sender_id: "u".into(),
            conversation_id: 1,
            connection_id: connection_id.into(),
            agent_type: AgentType::ClaudeCode,
            content_buffer: String::new(),
            tool_calls: Vec::new(),
            tool_call_inputs: HashMap::new(),
            delegation_rendered: HashSet::new(),
            last_flushed: Instant::now(),
            pending_prompt: None,
            permission_pending: None,
        }
    }

    async fn sent(rec: &Recorder) -> Vec<String> {
        rec.msgs.lock().await.clone()
    }

    /// A permission request from a NON-bridged (desktop / web) connection is
    /// pushed, carrying the localized title and the rendered operation detail.
    #[tokio::test]
    async fn permission_request_non_bridged_is_pushed() {
        let db = test_helpers::fresh_in_memory_db().await;
        let (chat, rec) = manager_with_recorder(7).await;
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));
        let config = config_all_on(7);
        let mut last_push = HashMap::new();

        process_envelope(
            &permission_envelope("desktop-conn"),
            &bridge,
            &chat,
            &db.conn,
            &config,
            &mut last_push,
        )
        .await;

        let msgs = sent(&rec).await;
        assert_eq!(msgs.len(), 1, "expected one push, got {msgs:?}");
        assert!(msgs[0].contains("Permission Request"), "got {:?}", msgs[0]);
        assert!(msgs[0].contains("Bash: npm test"), "got {:?}", msgs[0]);
    }

    /// A permission request from a chat-channel-bridged connection is suppressed
    /// here — the session relay owns the interactive flow for that channel.
    #[tokio::test]
    async fn permission_request_bridged_is_suppressed() {
        let db = test_helpers::fresh_in_memory_db().await;
        let (chat, rec) = manager_with_recorder(7).await;
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));
        bridge
            .lock()
            .await
            .register("im-conn".into(), bridged_session("im-conn", 7));
        let config = config_all_on(7);
        let mut last_push = HashMap::new();

        process_envelope(
            &permission_envelope("im-conn"),
            &bridge,
            &chat,
            &db.conn,
            &config,
            &mut last_push,
        )
        .await;

        assert!(
            sent(&rec).await.is_empty(),
            "bridged permission request must not be double-pushed"
        );
    }

    /// The global event filter still gates permission_request: with the id
    /// toggled off (filter present, not containing it), nothing is pushed.
    #[tokio::test]
    async fn permission_request_respects_global_filter() {
        let db = test_helpers::fresh_in_memory_db().await;
        let (chat, rec) = manager_with_recorder(7).await;
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));
        let mut config = config_all_on(7);
        config.global_filter = Some(vec!["turn_complete".to_string()]);
        let mut last_push = HashMap::new();

        process_envelope(
            &permission_envelope("desktop-conn"),
            &bridge,
            &chat,
            &db.conn,
            &config,
            &mut last_push,
        )
        .await;

        assert!(sent(&rec).await.is_empty());
    }

    /// Permission requests bypass the debounce: two distinct agents blocking on
    /// a gate back-to-back (well inside the 5s window) must BOTH be pushed,
    /// because a blocked agent emits no further event to re-trigger a lost nudge.
    #[tokio::test]
    async fn permission_requests_are_not_debounced() {
        let db = test_helpers::fresh_in_memory_db().await;
        let (chat, rec) = manager_with_recorder(7).await;
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));
        let config = config_all_on(7);
        let mut last_push = HashMap::new();

        process_envelope(
            &permission_envelope("conn-a"),
            &bridge,
            &chat,
            &db.conn,
            &config,
            &mut last_push,
        )
        .await;
        process_envelope(
            &permission_envelope("conn-b"),
            &bridge,
            &chat,
            &db.conn,
            &config,
            &mut last_push,
        )
        .await;

        assert_eq!(
            sent(&rec).await.len(),
            2,
            "both concurrent permission gates must be delivered"
        );
    }

    /// The reviewer's exact common path: ONE connection approves a first gate
    /// (e.g. ExitPlanMode) and immediately hits a second (e.g. Bash) well inside
    /// the 5s window. Both must deliver. Distinct from the test above, which
    /// covers two *different* connections — here the connection_id is identical,
    /// proving the exemption is connection-agnostic (debounce keys on
    /// (channel, event), so a blocked agent's follow-up gate is never swallowed).
    #[tokio::test]
    async fn permission_requests_same_connection_sequential_not_debounced() {
        let db = test_helpers::fresh_in_memory_db().await;
        let (chat, rec) = manager_with_recorder(7).await;
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));
        let config = config_all_on(7);
        let mut last_push = HashMap::new();

        let gate = |title: &str, raw: serde_json::Value| EventEnvelope {
            seq: 1,
            connection_id: "conn-a".into(),
            payload: AcpEvent::PermissionRequest {
                request_id: "req".into(),
                tool_call: serde_json::json!({ "title": title, "rawInput": raw }),
                options: vec![],
            },
        };

        process_envelope(
            &gate("ExitPlanMode", serde_json::json!({})),
            &bridge,
            &chat,
            &db.conn,
            &config,
            &mut last_push,
        )
        .await;
        process_envelope(
            &gate("Bash", serde_json::json!({ "command": "npm run build" })),
            &bridge,
            &chat,
            &db.conn,
            &config,
            &mut last_push,
        )
        .await;

        let msgs = sent(&rec).await;
        assert_eq!(
            msgs.len(),
            2,
            "both sequential gates on one connection must deliver, got {msgs:?}"
        );
    }

    /// Contrast: turn_complete is still debounced — a second one within the 5s
    /// window is dropped. Guards against accidentally exempting everything.
    #[tokio::test]
    async fn turn_complete_is_debounced_within_window() {
        let db = test_helpers::fresh_in_memory_db().await;
        let (chat, rec) = manager_with_recorder(7).await;
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));
        let config = config_all_on(7);
        let mut last_push = HashMap::new();

        let turn_complete = || EventEnvelope {
            seq: 1,
            connection_id: "c".into(),
            payload: AcpEvent::TurnComplete {
                session_id: "s".into(),
                stop_reason: "end_turn".into(),
                agent_type: "claude_code".into(),
            },
        };
        process_envelope(
            &turn_complete(),
            &bridge,
            &chat,
            &db.conn,
            &config,
            &mut last_push,
        )
        .await;
        process_envelope(
            &turn_complete(),
            &bridge,
            &chat,
            &db.conn,
            &config,
            &mut last_push,
        )
        .await;

        assert_eq!(
            sent(&rec).await.len(),
            1,
            "second turn_complete within 5s must be debounced"
        );
    }

    /// Regression: turn_complete (end_turn) still pushes after the refactor.
    #[tokio::test]
    async fn turn_complete_still_pushes() {
        let db = test_helpers::fresh_in_memory_db().await;
        let (chat, rec) = manager_with_recorder(7).await;
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));
        let config = config_all_on(7);
        let mut last_push = HashMap::new();

        let envelope = EventEnvelope {
            seq: 1,
            connection_id: "c".into(),
            payload: AcpEvent::TurnComplete {
                session_id: "s".into(),
                stop_reason: "end_turn".into(),
                agent_type: "claude_code".into(),
            },
        };
        process_envelope(&envelope, &bridge, &chat, &db.conn, &config, &mut last_push).await;

        assert_eq!(sent(&rec).await.len(), 1);
    }
}
