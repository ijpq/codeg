use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::models::{CodexQuotaSnapshot, CodexQuotaWindow};
use crate::parsers::codex::resolve_codex_home_dir;
use crate::parsers::ParseError;

/// Quota events are written beside the normal Codex token-count event near the
/// end of a rollout. Reading a bounded tail keeps the composer badge cheap even
/// for multi-hundred-megabyte sessions. Eight MiB is intentionally generous for
/// a final tool result while still avoiding a full transcript parse.
const CODEX_QUOTA_TAIL_BYTES: u64 = 8 * 1024 * 1024;

fn parse_quota_window(value: &serde_json::Value) -> Option<CodexQuotaWindow> {
    let used_percent = value.get("used_percent")?.as_f64()?;
    let window_minutes = value.get("window_minutes")?.as_u64()?;
    Some(CodexQuotaWindow {
        used_percent,
        window_minutes,
        resets_at: value.get("resets_at").and_then(|v| v.as_i64()),
    })
}

fn quota_snapshot_from_record(value: &serde_json::Value) -> Option<CodexQuotaSnapshot> {
    if value.get("type").and_then(|v| v.as_str()) != Some("event_msg") {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type").and_then(|v| v.as_str()) != Some("token_count") {
        return None;
    }
    let limits = payload.get("rate_limits")?.as_object()?;
    let plan_type = limits.get("plan_type")?.as_str()?.trim();
    if plan_type.is_empty() {
        return None;
    }

    let windows: Vec<CodexQuotaWindow> = ["primary", "secondary"]
        .into_iter()
        .filter_map(|key| limits.get(key).and_then(parse_quota_window))
        .collect();
    let weekly = windows
        .iter()
        .find(|window| window.window_minutes == 7 * 24 * 60)
        .cloned();
    let short_window = windows
        .iter()
        .filter(|window| window.window_minutes != 7 * 24 * 60)
        .min_by_key(|window| window.window_minutes)
        .cloned();

    // A plan without a usable window is not useful to the quota badge and is
    // often an initialization placeholder. Wait for the next real observation.
    if weekly.is_none() && short_window.is_none() {
        return None;
    }

    Some(CodexQuotaSnapshot {
        plan_type: plan_type.to_string(),
        limit_id: limits
            .get("limit_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        limit_name: limits
            .get("limit_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        weekly,
        short_window,
        observed_at: value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

fn quota_snapshot_from_rollout(path: &Path) -> Result<Option<CodexQuotaSnapshot>, ParseError> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(CODEX_QUOTA_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;

    let mut reader = BufReader::new(file);
    if start > 0 {
        // The seek likely landed in the middle of a JSONL record. Discard only
        // that fragment; every following line is a complete event.
        let mut partial = String::new();
        reader.read_line(&mut partial)?;
    }

    let mut latest = None;
    for line in reader.lines() {
        let line = match line {
            Ok(line) if line.contains("\"rate_limits\"") => line,
            _ => continue,
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(snapshot) = quota_snapshot_from_record(&value) {
            latest = Some(snapshot);
        }
    }
    Ok(latest)
}

/// Read the latest quota observation for one Codex session. With no session id,
/// use the newest rollout as a draft/new-conversation fallback. No network
/// request is made: the relay's metadata is consumed from Codex's own JSONL.
pub(crate) fn latest_quota_snapshot(
    conversation_id: Option<&str>,
) -> Result<Option<CodexQuotaSnapshot>, ParseError> {
    let sessions = resolve_codex_home_dir().join("sessions");
    if !sessions.exists() {
        return Ok(None);
    }

    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = WalkDir::new(sessions)
        .into_iter()
        .filter_map(Result::ok)
        .map(|entry| entry.into_path())
        .filter(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("rollout-")
                            && conversation_id.is_none_or(|id| name.contains(id))
                    })
        })
        .filter_map(|path| {
            let modified = path.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .collect();
    candidates.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));

    for (_, path) in candidates.into_iter().take(8) {
        if let Some(snapshot) = quota_snapshot_from_rollout(&path)? {
            return Ok(Some(snapshot));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{quota_snapshot_from_record, quota_snapshot_from_rollout};
    use std::env;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_codex_weekly_and_short_quota_windows() {
        let record = serde_json::json!({
            "timestamp": "2026-08-20T10:00:00Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "rate_limits": {
                    "limit_id": "codex",
                    "limit_name": "PRO 50x",
                    "primary": {
                        "used_percent": 37.5,
                        "window_minutes": 10080,
                        "resets_at": 1787220000
                    },
                    "secondary": {
                        "used_percent": 12,
                        "window_minutes": 300,
                        "resets_at": 1787200000
                    },
                    "plan_type": "pro"
                }
            }
        });

        let snapshot = quota_snapshot_from_record(&record).expect("quota snapshot");
        assert_eq!(snapshot.plan_type, "pro");
        assert_eq!(snapshot.limit_name.as_deref(), Some("PRO 50x"));
        assert_eq!(snapshot.weekly.as_ref().map(|w| w.used_percent), Some(37.5));
        assert_eq!(
            snapshot.short_window.as_ref().map(|w| w.window_minutes),
            Some(300)
        );
        assert_eq!(
            snapshot.observed_at.as_deref(),
            Some("2026-08-20T10:00:00Z")
        );
    }

    #[test]
    fn rollout_quota_reader_uses_last_valid_observation() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let path = env::temp_dir().join(format!("codeg-codex-quota-{nanos}.jsonl"));
        let content = concat!(
            "{\"timestamp\":\"2026-08-20T09:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"limit_id\":\"codex\",\"primary\":{\"used_percent\":20,\"window_minutes\":10080,\"resets_at\":1787220000},\"secondary\":null,\"plan_type\":\"plus\"}}}\n",
            "{\"timestamp\":\"2026-08-20T09:01:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"rate_limits is only prose here\"}}\n",
            "{\"timestamp\":\"2026-08-20T10:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"limit_id\":\"codex\",\"primary\":{\"used_percent\":64,\"window_minutes\":10080,\"resets_at\":1787221000},\"secondary\":null,\"plan_type\":\"prolite\"}}}\n"
        );
        fs::write(&path, content).expect("write quota fixture");

        let snapshot = quota_snapshot_from_rollout(&path)
            .expect("read quota fixture")
            .expect("latest quota");
        assert_eq!(snapshot.plan_type, "prolite");
        assert_eq!(snapshot.weekly.map(|w| w.used_percent), Some(64.0));

        let _ = fs::remove_file(path);
    }
}
