use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    Lark,
    Telegram,
    Weixin,
}

// ── Per-channel strong typed configs ──

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    pub chat_id: String,
    #[serde(default)]
    pub topic_mode: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LarkConfig {
    pub app_id: String,
    pub chat_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WeixinConfig {
    pub base_url: String,
    #[serde(default)]
    pub default_folder_id: Option<i32>,
    #[serde(default)]
    pub default_agent_type: Option<String>,
    #[serde(default)]
    pub default_conversation_id: Option<i32>,
    /// Server-enforced outbound policy. Missing values intentionally default
    /// to final results plus user-blocking interactions for existing installs.
    #[serde(default)]
    pub push_mode: WeixinPushMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeixinPushMode {
    FinalOnly,
    #[default]
    FinalAndInteractions,
    Debug,
}

impl WeixinPushMode {
    pub fn from_config_json(config_json: &str) -> Self {
        serde_json::from_str::<serde_json::Value>(config_json)
            .ok()
            .and_then(|value| value.get("push_mode").cloned())
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    pub fn allows_progress(self) -> bool {
        self == Self::Debug
    }

    pub fn allows_interactions(self) -> bool {
        matches!(self, Self::FinalAndInteractions | Self::Debug)
    }
}

/// A stable, non-reversible identifier suitable for routing logs. Never log
/// raw provider sender ids: they are credentials-adjacent personal data.
pub fn sender_log_key(sender_id: &str) -> String {
    let digest = Sha256::digest(sender_id.as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelType::Lark => write!(f, "lark"),
            ChannelType::Telegram => write!(f, "telegram"),
            ChannelType::Weixin => write!(f, "weixin"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelConnectionStatus {
    Connected,
    Connecting,
    Disconnected,
    Error,
}

#[derive(Debug, Clone)]
pub struct SentMessageId(pub String);

#[derive(Debug, Clone)]
pub enum DeliveryOutcome {
    Delivered {
        message_id: SentMessageId,
        context_generation: Option<u64>,
    },
    DeferredContextExpired {
        context_generation: Option<u64>,
    },
}

pub struct IncomingCommand {
    pub channel_id: i32,
    pub sender_id: String,
    pub command_text: String,
    pub callback_data: Option<String>,
    pub target: ChannelMessageTarget,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelMessageTarget {
    pub channel_id: i32,
    pub chat_id: Option<String>,
    pub thread_key: Option<String>,
    pub thread_kind: Option<String>,
    pub provider_payload: Option<serde_json::Value>,
}

impl ChannelMessageTarget {
    pub fn channel(channel_id: i32) -> Self {
        Self {
            channel_id,
            chat_id: None,
            thread_key: None,
            thread_kind: None,
            provider_payload: None,
        }
    }

    pub fn telegram_general(channel_id: i32, chat_id: impl Into<String>) -> Self {
        Self {
            channel_id,
            chat_id: Some(chat_id.into()),
            thread_key: None,
            thread_kind: Some(TELEGRAM_GENERAL_THREAD_KIND.to_string()),
            provider_payload: None,
        }
    }

    pub fn telegram_forum_topic(
        channel_id: i32,
        chat_id: impl Into<String>,
        thread_key: impl Into<String>,
    ) -> Self {
        Self {
            channel_id,
            chat_id: Some(chat_id.into()),
            thread_key: Some(thread_key.into()),
            thread_kind: Some(TELEGRAM_FORUM_THREAD_KIND.to_string()),
            provider_payload: None,
        }
    }

    pub fn is_telegram_forum_topic(&self) -> bool {
        self.thread_kind.as_deref() == Some(TELEGRAM_FORUM_THREAD_KIND) && self.thread_key.is_some()
    }

    pub fn is_telegram_general_topic(&self) -> bool {
        self.thread_kind.as_deref() == Some(TELEGRAM_GENERAL_THREAD_KIND)
    }

    pub fn matches_thread(&self, other: &Self) -> bool {
        self.channel_id == other.channel_id
            && self.chat_id == other.chat_id
            && self.thread_kind == other.thread_kind
            && self.thread_key == other.thread_key
    }
}

pub const TELEGRAM_FORUM_THREAD_KIND: &str = "telegram_forum_topic";
pub const TELEGRAM_GENERAL_THREAD_KIND: &str = "telegram_general_topic";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct RichMessage {
    pub title: Option<String>,
    pub body: String,
    pub fields: Vec<(String, String)>,
    pub level: MessageLevel,
}

impl RichMessage {
    pub fn info(body: impl Into<String>) -> Self {
        Self {
            title: None,
            body: body.into(),
            fields: Vec::new(),
            level: MessageLevel::Info,
        }
    }

    pub fn error(body: impl Into<String>) -> Self {
        Self {
            title: None,
            body: body.into(),
            fields: Vec::new(),
            level: MessageLevel::Error,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.push((key.into(), value.into()));
        self
    }

    pub fn to_plain_text(&self) -> String {
        let mut text = String::new();
        if let Some(title) = &self.title {
            text.push_str(title);
            text.push('\n');
        }
        text.push_str(&self.body);
        for (key, value) in &self.fields {
            text.push_str(&format!("\n{}: {}", key, value));
        }
        text
    }
}

// ── Phase 2 forward-compatible types ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonStyle {
    Primary,
    Danger,
    Default,
}

#[derive(Debug, Clone)]
pub struct MessageButton {
    pub id: String,
    pub label: String,
    pub style: ButtonStyle,
}

#[derive(Debug, Clone)]
pub struct InteractiveMessage {
    pub base: RichMessage,
    pub buttons: Vec<MessageButton>,
    pub callback_context: serde_json::Value,
}

impl InteractiveMessage {
    pub fn to_rich_fallback(&self) -> RichMessage {
        let mut msg = self.base.clone();
        if !self.buttons.is_empty() {
            let button_text: Vec<String> = self
                .buttons
                .iter()
                .map(|b| format!("[{}]", b.label))
                .collect();
            msg.body
                .push_str(&format!("\n\n{}", button_text.join("  ")));
        }
        msg
    }
}
