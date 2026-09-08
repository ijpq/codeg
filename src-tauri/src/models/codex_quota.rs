use serde::{Deserialize, Serialize};

/// One rolling Codex allowance window reported by the upstream service.
///
/// `used_percent` is deliberately kept as reported instead of being rounded in
/// the backend. The composer derives the remaining percentage for display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexQuotaWindow {
    pub used_percent: f64,
    pub window_minutes: u64,
    pub resets_at: Option<i64>,
}

/// Most recent quota observation carried by a real Codex model response.
///
/// This contains no account identifier or credential. `plan_type` and the
/// optional limit name are the anonymous subscription/pool labels already
/// returned by Codex (or by a compatible relay).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexQuotaSnapshot {
    pub plan_type: String,
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub weekly: Option<CodexQuotaWindow>,
    pub short_window: Option<CodexQuotaWindow>,
    pub observed_at: Option<String>,
}
