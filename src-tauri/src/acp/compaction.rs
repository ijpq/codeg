//! Capture the original business/session identity before a turn or connection
//! can be retired. Only rare, versioned Pi lifecycle frames enter persistence.
use crate::acp::{session_state::SessionState, types::AcpEvent};
use crate::models::AgentType;
use serde_json::{json, Value};

pub(crate) const BINDING_KEY: &str = "codeg.compactionBinding";

pub(crate) fn lifecycle_meta(event: &AcpEvent) -> Option<&Value> {
    let meta = match event {
        AcpEvent::ToolCall { meta, .. } | AcpEvent::ToolCallUpdate { meta, .. } => meta.as_ref()?,
        _ => return None,
    };
    let marker = meta.get("contextCompaction")?;
    let status = marker.get("status")?.as_str()?;
    (marker.get("version")?.as_u64()? >= 1
        && matches!(status, "in_progress" | "completed" | "failed" | "cancelled")
        && meta.get(BINDING_KEY).is_some())
    .then_some(meta)
}

pub(crate) fn stamp_binding(event: &mut AcpEvent, state: &SessionState) {
    let (id, Some(meta)) = (match event {
        AcpEvent::ToolCall {
            tool_call_id, meta, ..
        }
        | AcpEvent::ToolCallUpdate {
            tool_call_id, meta, ..
        } => (tool_call_id, meta),
        _ => return,
    }) else {
        return;
    };
    // Treat the ownership stamp as CodeG data, never adapter-supplied data.
    if let Some(fields) = meta.as_object_mut() {
        fields.remove(BINDING_KEY);
    }
    if state.agent_type != AgentType::Pi {
        return;
    }
    let (Some(cid), Some(external)) = (state.conversation_id, &state.external_id) else {
        return;
    };
    let Some(marker) = meta
        .get_mut("contextCompaction")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if marker.get("version").and_then(Value::as_u64).unwrap_or(0) < 1 {
        return;
    }
    // Failed/cancelled operations have no native JSONL entry. Their ACP id is
    // the durable event identity; successful operations retain the proven
    // native entry id supplied by the adapter.
    marker.entry("eventId").or_insert_with(|| json!(id));
    if let Some(meta) = meta.as_object_mut() {
        meta.insert(
            BINDING_KEY.into(),
            json!({
                "conversationId": cid,
                "externalId": external,
                "connectionId": state.connection_id,
                "turnRunId": state.active_turn_run_id,
                "observedAt": chrono::Utc::now().timestamp_millis(),
            }),
        );
    }
}
