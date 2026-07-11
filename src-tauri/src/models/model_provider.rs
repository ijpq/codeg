use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProviderInfo {
    pub id: i32,
    pub name: String,
    pub api_url: String,
    pub api_key: String,
    pub api_key_masked: String,
    pub agent_type: String,
    pub model: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn mask_api_key(key: &str) -> String {
    // Operate on Unicode scalar values, not bytes: an API key may contain a
    // multibyte character (e.g. a full-width char typed with a CJK IME), and
    // byte-slicing `&key[..4]` would panic on a non-char-boundary. Such a panic
    // in `From` propagates out of every `list_model_providers` call once the
    // row is persisted, permanently breaking the provider list.
    let chars: Vec<char> = key.chars().collect();
    let len = chars.len();
    if len <= 8 {
        "\u{2022}".repeat(len)
    } else {
        let prefix: String = chars[..4].iter().collect();
        let suffix: String = chars[len - 4..].iter().collect();
        format!("{}{}{}", prefix, "\u{2022}".repeat(len.min(20) - 8), suffix)
    }
}

impl From<crate::db::entities::model_provider::Model> for ModelProviderInfo {
    fn from(m: crate::db::entities::model_provider::Model) -> Self {
        let agent_type = if m.agent_type.is_empty() {
            // Fallback for rows backfilled with empty agent_type.
            serde_json::from_str::<Vec<String>>(&m.agent_types_json)
                .ok()
                .and_then(|list| list.into_iter().next())
                .unwrap_or_default()
        } else {
            m.agent_type
        };
        Self {
            id: m.id,
            name: m.name,
            api_url: m.api_url,
            api_key: m.api_key.clone(),
            api_key_masked: mask_api_key(&m.api_key),
            agent_type,
            model: m.model,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.updated_at.to_rfc3339(),
        }
    }
}

/// Result of probing a model provider's `api_url` reachability. Drives the
/// "test link" button in the streaming-diagnostics panel, which needs to tell a
/// stalled network path (CF tunnel / VPS unreachable) apart from a model that is
/// simply still working — codeg can't see the subprocess's model HTTP traffic,
/// so it issues its own lightweight probe instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProviderProbeResult {
    /// False when the agent has no custom provider bound (official/direct
    /// connection) — there is nothing to probe.
    pub configured: bool,
    /// True when the endpoint returned any HTTP response (even 4xx/5xx): the
    /// tunnel + VPS are alive. False on connect/timeout/DNS errors.
    pub reachable: bool,
    /// HTTP status code of the response, when one was received.
    pub status: Option<u16>,
    /// Round-trip latency in milliseconds, when a response was received.
    pub latency_ms: Option<u64>,
    /// The provider's configured base URL (for display), when configured.
    pub api_url: Option<String>,
    /// Error string on a failed probe (connection refused / timeout / DNS).
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::mask_api_key;

    #[test]
    fn masks_short_ascii_key() {
        assert_eq!(mask_api_key("abc123"), "\u{2022}".repeat(6));
    }

    #[test]
    fn masks_long_ascii_key_keeping_edges() {
        assert_eq!(mask_api_key("sk-test-1234567890"), "sk-t\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}7890");
    }

    #[test]
    fn does_not_panic_on_multibyte_key() {
        // Byte index 4 falls inside '密' (bytes 3..6); a byte slice would panic.
        let masked = mask_api_key("sk-密钥abcd1234");
        assert!(masked.starts_with("sk-密"));
        assert!(masked.ends_with("1234"));
    }

    #[test]
    fn masks_short_multibyte_key_without_panic() {
        assert_eq!(mask_api_key("密钥abc"), "\u{2022}".repeat(5));
    }
}
