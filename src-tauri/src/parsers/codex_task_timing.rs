//! Codex times a whole task, not each model request or tool exchange. Match
//! native start/end records by turn_id and publish that duration exactly once.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::message::{MessageTurn, TurnRole};

#[derive(Default)]
struct Timing {
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    duration_ms: Option<u64>,
    finished: bool,
}

#[derive(Default)]
pub(super) struct CodexTaskTimings {
    current: Option<String>,
    tasks: HashMap<String, Timing>,
}

fn task_id(payload: &Value) -> Option<&str> {
    payload.get("turn_id")?.as_str().filter(|id| !id.is_empty())
}

fn native_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?;
    value
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| DateTime::from_timestamp(value.as_i64()?, 0))
}

impl CodexTaskTimings {
    pub(super) fn current_id(&self) -> Option<&str> {
        self.current.as_deref()
    }

    pub(super) fn start(&mut self, payload: &Value, time: Option<DateTime<Utc>>) {
        self.current = task_id(payload).map(str::to_owned);
        if let Some(id) = &self.current {
            // A repeated marker cannot move the start forward mid-task.
            let timing = self.tasks.entry(id.clone()).or_default();
            if timing.start.is_none() {
                timing.start = time;
            }
        }
    }

    pub(super) fn context(&mut self, payload: Option<&Value>) {
        // A bounded history page can open on turn_context, immediately before
        // its prompt. Its id is usable; its timestamp is not a task start.
        if self.current.is_none() {
            self.current = payload
                .and_then(task_id)
                .filter(|id| !self.tasks.get(*id).is_some_and(|task| task.finished))
                .map(str::to_owned);
        }
    }

    pub(super) fn finish(&mut self, payload: &Value, time: Option<DateTime<Utc>>) {
        let Some(id) = task_id(payload) else {
            // Older interrupt records omit identity. Close the active scope so
            // subsequent notices cannot inherit it, but invent no elapsed time.
            if let Some(id) = self.current.take() {
                self.tasks.entry(id).or_default().finished = true;
            }
            return;
        };
        let timing = self.tasks.entry(id.to_owned()).or_default();
        let start = timing
            .start
            .or_else(|| native_time(payload.get("started_at")));
        // Preserve record precision for fallback; native epoch seconds may be
        // truncated. Native duration_ms is independent of those rounded clocks.
        timing.end = time.or_else(|| native_time(payload.get("completed_at")));
        timing.duration_ms = payload
            .get("duration_ms")
            .and_then(Value::as_u64)
            .or(timing.duration_ms)
            .or_else(|| {
                let ms = (timing.end? - start?).num_milliseconds();
                u64::try_from(ms).ok()
            });
        timing.finished = true;
        if self.current.as_deref() == Some(id) {
            self.current = None;
        }
    }

    pub(super) fn apply(&self, turns: &mut [MessageTurn]) {
        let mut last_assistants = HashMap::new();
        for (index, turn) in turns.iter().enumerate() {
            if matches!(turn.role, TurnRole::Assistant) {
                if let Some(id) = &turn.native_task_id {
                    last_assistants.insert(id.clone(), index);
                }
            }
        }
        for (id, index) in last_assistants {
            if let Some(timing) = self.tasks.get(&id) {
                let turn = &mut turns[index];
                turn.duration_ms = timing.duration_ms;
                if timing.end.is_some() {
                    turn.completed_at = timing.end;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn clock(s: &str) -> Option<DateTime<Utc>> {
        Some(s.parse().unwrap())
    }

    #[test]
    fn matches_terminal_records_by_id_and_prefers_native_milliseconds() {
        let mut tasks = CodexTaskTimings::default();
        tasks.start(&json!({"turn_id":"a"}), clock("2026-10-19T10:00:00Z"));
        tasks.start(&json!({"turn_id":"b"}), clock("2026-10-19T11:00:00Z"));
        tasks.finish(
            &json!({"turn_id":"a", "duration_ms":168795}),
            clock("2026-10-19T10:02:49Z"),
        );
        assert_eq!(tasks.current_id(), Some("b"));
        tasks.finish(
            &json!({"turn_id":"b", "duration_ms":615361}),
            clock("2026-10-19T11:10:16Z"),
        );
        assert_eq!(tasks.tasks["a"].duration_ms, Some(168795));
        assert_eq!(tasks.tasks["b"].duration_ms, Some(615361));
        tasks.context(Some(&json!({"turn_id":"b"})));
        assert_eq!(
            tasks.current_id(),
            None,
            "a finished task cannot time a branch notice"
        );
        tasks.finish(&json!({"turn_id":"a"}), clock("2026-10-19T10:02:50Z"));
        assert_eq!(tasks.tasks["a"].duration_ms, Some(168795));
    }

    #[test]
    fn falls_back_only_to_matching_trustworthy_start_and_end() {
        let mut tasks = CodexTaskTimings::default();
        tasks.start(
            &json!({"turn_id":"cancelled"}),
            clock("2026-10-19T10:00:00.100Z"),
        );
        tasks.context(Some(&json!({"turn_id":"cancelled"})));
        tasks.finish(
            &json!({"turn_id":"cancelled"}),
            clock("2026-10-19T10:00:09.300Z"),
        );
        assert_eq!(tasks.tasks["cancelled"].duration_ms, Some(9200));
        tasks.context(Some(&json!({"turn_id":"no-start"})));
        tasks.finish(
            &json!({"turn_id":"no-start"}),
            clock("2026-10-19T10:10:00Z"),
        );
        assert_eq!(tasks.tasks["no-start"].duration_ms, None);
        tasks.start(
            &json!({"turn_id":"backwards"}),
            clock("2026-10-19T11:00:00Z"),
        );
        tasks.finish(
            &json!({"turn_id":"backwards"}),
            clock("2026-10-19T10:00:00Z"),
        );
        assert_eq!(tasks.tasks["backwards"].duration_ms, None);
        tasks.finish(
            &json!({"turn_id":"native-failure", "duration_ms":0, "error":{"message":"failed"}}),
            None,
        );
        assert_eq!(tasks.tasks["native-failure"].duration_ms, Some(0));
        tasks.finish(
            &json!({"turn_id":"payload-clocks", "started_at":100, "completed_at":105}),
            None,
        );
        assert_eq!(tasks.tasks["payload-clocks"].duration_ms, Some(5000));
    }
}
