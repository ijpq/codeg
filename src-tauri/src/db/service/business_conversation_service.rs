//! Engine-independent identity, durable history, handoff cursors and receipts.
//! Native transcripts and credentials remain exclusively engine-owned.

use crate::db::error::DbError;
use crate::models::conversation::ConversationOutputTurnRef;
use crate::models::{AgentType, DbConversationSummary, MessageTurn};
use chrono::Utc;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, QueryResult, Statement, TransactionTrait, Value,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn stmt(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, sql, values)
}

fn json<T: Serialize>(value: &T) -> Result<String, DbError> {
    serde_json::to_string(value).map_err(|e| DbError::Validation(e.to_string()))
}

fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, DbError> {
    serde_json::from_str(value).map_err(|e| DbError::Validation(e.to_string()))
}

pub fn agent_key(agent: AgentType) -> Result<String, DbError> {
    Ok(json(&agent)?.trim_matches('"').to_owned())
}

pub fn supports_main_agent_handoff(agent: AgentType) -> bool {
    matches!(
        agent,
        AgentType::Codex | AgentType::ClaudeCode | AgentType::Pi
    )
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BusinessTaskState {
    pub objective: String,
    pub requirements: Vec<String>,
    pub completed: Vec<String>,
    pub pending: Vec<String>,
    pub latest_files: Vec<String>,
    pub decisions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSwitchRequest {
    pub conversation_id: i32,
    pub request_id: String,
    pub target_agent: AgentType,
    #[serde(default)]
    pub expected_revision: Option<i64>,
    #[serde(default)]
    pub preferred_mode_id: Option<String>,
    #[serde(default)]
    pub preferred_config_values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessAgentSession {
    pub id: i64,
    pub agent_type: AgentType,
    pub external_id: String,
    pub mode_id: Option<String>,
    pub config_values: BTreeMap<String, String>,
    pub seen_sequence: i64,
    /// Most recent history boundary carried by an accepted bounded bridge.
    /// Separate from the contiguous cursor so a declared older gap never
    /// forces the whole original transcript into the model before Goal use.
    #[serde(skip)]
    pub last_bridge_sequence: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessConversationState {
    pub conversation_id: i32,
    pub revision: i64,
    pub active_agent: AgentType,
    pub pending_request_id: Option<String>,
    pub pending_agent: Option<AgentType>,
    pub switch_status: Option<String>,
    pub switch_error: Option<String>,
    pub switch_phase: Option<String>,
    pub task_state: BusinessTaskState,
    pub sessions: Vec<BusinessAgentSession>,
    pub last_sequence: i64,
}

fn session(row: QueryResult) -> Result<BusinessAgentSession, DbError> {
    let key: String = row.try_get("", "agent_type")?;
    Ok(BusinessAgentSession {
        id: row.try_get("", "id")?,
        agent_type: decode(&format!("\"{key}\""))?,
        external_id: row.try_get("", "external_id")?,
        mode_id: row.try_get("", "mode_id")?,
        config_values: decode(&row.try_get::<String>("", "config_values")?)?,
        seen_sequence: row.try_get("", "seen_sequence")?,
        last_bridge_sequence: row.try_get("", "last_bridge_sequence")?,
    })
}

pub async fn is_managed(conn: &DatabaseConnection, cid: i32) -> Result<bool, DbError> {
    Ok(conn.query_one(stmt("SELECT conversation_id FROM business_conversation_state WHERE conversation_id=? AND enabled=1",vec![cid.into()])).await?.is_some())
}

pub async fn state(
    conn: &DatabaseConnection,
    cid: i32,
) -> Result<Option<BusinessConversationState>, DbError> {
    // A single SQLite read snapshot prevents mixing an old projection/revision
    // with the final receipt from a concurrent activation on another client.
    let tx = conn.begin().await?;
    let conn = &tx;
    let Some(row)=conn.query_one(stmt("SELECT b.*,c.agent_type FROM business_conversation_state b JOIN conversation c ON c.id=b.conversation_id WHERE b.conversation_id=? AND b.enabled=1",vec![cid.into()])).await? else {return Ok(None)};
    let pending: Option<String> = row.try_get("", "pending_request_id")?;
    let request = if let Some(id) = &pending {
        conn.query_one(stmt(
            "SELECT * FROM business_agent_switch WHERE request_id=?",
            vec![id.clone().into()],
        ))
        .await?
    } else {
        None
    };
    let pending_agent = match request.as_ref() {
        Some(r) => Some(
            decode::<AgentSwitchRequest>(&r.try_get::<String>("", "request_json")?)?.target_agent,
        ),
        None => None,
    };
    let sessions = conn
        .query_all(stmt(
            "SELECT * FROM conversation_agent_session WHERE conversation_id=? ORDER BY id",
            vec![cid.into()],
        ))
        .await?
        .into_iter()
        .map(session)
        .collect::<Result<Vec<_>, _>>()?;
    let last = conn
        .query_one(stmt(
            "SELECT COALESCE(MAX(sequence),0) AS n FROM business_message WHERE conversation_id=?",
            vec![cid.into()],
        ))
        .await?
        .unwrap()
        .try_get("", "n")?;
    let result = BusinessConversationState {
        conversation_id: cid,
        revision: row.try_get("", "revision")?,
        active_agent: decode(&format!("\"{}\"", row.try_get::<String>("", "agent_type")?))?,
        pending_request_id: pending,
        pending_agent,
        switch_status: request
            .as_ref()
            .map(|r| r.try_get("", "status"))
            .transpose()?,
        switch_error: request
            .as_ref()
            .map(|r| r.try_get("", "error"))
            .transpose()?
            .flatten(),
        switch_phase: request
            .as_ref()
            .map(|r| r.try_get("", "phase"))
            .transpose()?
            .flatten(),
        task_state: decode(&row.try_get::<String>("", "task_state")?)?,
        sessions,
        last_sequence: last,
    };
    tx.commit().await?;
    Ok(Some(result))
}

pub async fn initialize(
    conn: &DatabaseConnection,
    summary: &DbConversationSummary,
    turns: &[MessageTurn],
) -> Result<(), DbError> {
    let external = summary
        .external_id
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            DbError::Validation("The original conversation has no durable session".into())
        })?;
    let tx = conn.begin().await?;
    tx.execute(stmt("INSERT INTO business_conversation_state(conversation_id,original_agent,original_external_id,original_model) VALUES(?,?,?,?) ON CONFLICT(conversation_id) DO NOTHING",vec![summary.id.into(),agent_key(summary.agent_type)?.into(),external.clone().into(),summary.model.clone().into()])).await?;
    tx.execute(stmt("INSERT INTO conversation_agent_session(conversation_id,agent_type,external_id) VALUES(?,?,?) ON CONFLICT(conversation_id,agent_type) DO NOTHING",vec![summary.id.into(),agent_key(summary.agent_type)?.into(),external.into()])).await?;
    let sid: i64 = tx
        .query_one(stmt(
            "SELECT id FROM conversation_agent_session WHERE conversation_id=? AND agent_type=?",
            vec![summary.id.into(), agent_key(summary.agent_type)?.into()],
        ))
        .await?
        .unwrap()
        .try_get("", "id")?;
    persist_turns(&tx, summary.id, sid, turns).await?;
    tx.execute(stmt(
        "UPDATE business_conversation_state SET enabled=1 WHERE conversation_id=?",
        vec![summary.id.into()],
    ))
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn persist_turns<C: ConnectionTrait>(
    conn: &C,
    cid: i32,
    sid: i64,
    turns: &[MessageTurn],
) -> Result<(), DbError> {
    persist_turns_with_refs(conn, cid, sid, turns, &[]).await
}

#[derive(Serialize, Deserialize)]
struct StoredBusinessTurn {
    #[serde(flatten)]
    turn: MessageTurn,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codeg_prompt_fingerprint: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct BusinessHistoryArchivePage {
    pub turns: Vec<MessageTurn>,
    #[serde(default)]
    pub output_turn_refs: Vec<ConversationOutputTurnRef>,
}

async fn persist_turns_with_refs<C: ConnectionTrait>(
    conn: &C,
    cid: i32,
    sid: i64,
    turns: &[MessageTurn],
    refs: &[ConversationOutputTurnRef],
) -> Result<(), DbError> {
    let external: String = conn
        .query_one(stmt(
            "SELECT external_id FROM conversation_agent_session WHERE id=? AND conversation_id=?",
            vec![sid.into(), cid.into()],
        ))
        .await?
        .ok_or_else(|| DbError::Validation("Business session is missing".into()))?
        .try_get("", "external_id")?;
    let mut inserted = 0_i64;
    for turn in turns {
        // /clear and native rollover may restart a parser's turn counter.
        // The original transcript identity is part of every canonical key.
        let native_key = format!("{external}:{}", turn.id);
        let role = match turn.role {
            crate::models::message::TurnRole::User => "user",
            crate::models::message::TurnRole::Assistant => "assistant",
            crate::models::message::TurnRole::System => "system",
        };
        // Never replace a completed native duration with a partial parse.
        let previous=conn.query_one(stmt("SELECT turn_json FROM business_message WHERE conversation_id=? AND session_id=? AND native_turn_id=?",vec![cid.into(),sid.into(),native_key.clone().into()])).await?;
        let mut incoming = turn.clone();
        let mut fingerprint = refs
            .iter()
            .find(|reference| reference.id == turn.id)
            .and_then(|reference| reference.prompt_fingerprint.clone());
        // Deferred images no longer contain the bytes used for the accepted
        // prompt hash. Preserve the pre-projection identity, never hash a
        // preview as if it were the original attachment.
        if fingerprint.is_none() {
            fingerprint = crate::artifact_tracker::user_turn_fingerprint(turn);
        }
        if previous.is_none() {
            inserted += 1;
        }
        if let Some(previous) = previous {
            let stored: StoredBusinessTurn = decode(&previous.try_get::<String>("", "turn_json")?)?;
            if fingerprint.is_none() {
                fingerprint = stored.codeg_prompt_fingerprint;
            }
            let old = stored.turn;
            if incoming.duration_ms.is_none() {
                incoming.duration_ms = old.duration_ms;
            }
            if incoming.usage.is_none() {
                incoming.usage = old.usage;
            }
            if incoming.completed_at.is_none() {
                incoming.completed_at = old.completed_at;
            }
        }
        let stored = StoredBusinessTurn {
            turn: incoming,
            codeg_prompt_fingerprint: fingerprint,
        };
        conn.execute(stmt("INSERT INTO business_message(conversation_id,session_id,native_turn_id,role,turn_json) VALUES(?,?,?,?,?) ON CONFLICT(conversation_id,session_id,native_turn_id) DO UPDATE SET turn_json=excluded.turn_json",vec![cid.into(),sid.into(),native_key.into(),role.into(),json(&stored)?.into()])).await?;
    }
    conn.execute(stmt("UPDATE business_conversation_state SET message_count=message_count+? WHERE conversation_id=?",vec![inserted.into(),cid.into()])).await?;
    conn.execute(stmt("UPDATE conversation SET message_count=(SELECT message_count FROM business_conversation_state WHERE conversation_id=?) WHERE id=?",vec![cid.into(),cid.into()])).await?;
    Ok(())
}

pub async fn queue_switch(
    conn: &DatabaseConnection,
    request: &AgentSwitchRequest,
) -> Result<(), DbError> {
    if request.request_id.trim().is_empty() || request.request_id.len() > 128 {
        return Err(DbError::Validation("Invalid switch request ID".into()));
    }
    if !supports_main_agent_handoff(request.target_agent) {
        return Err(DbError::Validation(
            "Main-agent handoff currently supports Codex, Claude Code and Pi".into(),
        ));
    }
    let payload = json(request)?;
    let tx = conn.begin().await?;
    // Claim SQLite's writer before reading the retry receipt. A deferred
    // read snapshot cannot be promoted if streaming history writes race it;
    // busy_timeout cannot recover that promotion. Do not advance the CAS
    // revision or clocks for this claim or for an idempotent retry.
    tx.execute(stmt(
        "UPDATE business_conversation_state SET revision=revision WHERE conversation_id=?",
        vec![request.conversation_id.into()],
    ))
    .await?;
    if let Some(old) = tx
        .query_one(stmt(
            "SELECT request_json FROM business_agent_switch WHERE request_id=?",
            vec![request.request_id.clone().into()],
        ))
        .await?
    {
        if old.try_get::<String>("", "request_json")? != payload {
            return Err(DbError::Conflict(
                "Switch request ID reused with different parameters".into(),
            ));
        }
        tx.commit().await?;
        return Ok(());
    }
    let expected = request.expected_revision;
    let result=tx.execute(stmt("UPDATE business_conversation_state SET pending_request_id=?,revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE conversation_id=? AND enabled=1 AND (? IS NULL OR revision=?)",vec![request.request_id.clone().into(),request.conversation_id.into(),expected.into(),expected.into()])).await?;
    if result.rows_affected() != 1 {
        return Err(DbError::Conflict(
            "Conversation changed; refresh its agent state and retry".into(),
        ));
    }
    tx.execute(stmt("UPDATE business_agent_switch SET status='superseded',updated_at=CURRENT_TIMESTAMP WHERE conversation_id=? AND status IN ('queued','preparing')",vec![request.conversation_id.into()])).await?;
    tx.execute(stmt(
        "INSERT INTO business_agent_switch(request_id,conversation_id,request_json) VALUES(?,?,?)",
        vec![
            request.request_id.clone().into(),
            request.conversation_id.into(),
            payload.into(),
        ],
    ))
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn is_switch_retry(
    conn: &DatabaseConnection,
    request: &AgentSwitchRequest,
) -> Result<bool, DbError> {
    Ok(conn.query_one(stmt("SELECT request_id FROM business_agent_switch WHERE request_id=? AND conversation_id=? AND request_json=?",vec![request.request_id.clone().into(),request.conversation_id.into(),json(request)?.into()])).await?.is_some())
}

pub async fn persist_current_tail(
    conn: &DatabaseConnection,
    cid: i32,
    sid: i64,
    agent: AgentType,
    external: &str,
    turns: &[MessageTurn],
    refs: &[ConversationOutputTurnRef],
) -> Result<bool, DbError> {
    let tx = conn.begin().await?;
    let current=tx.execute(stmt("UPDATE conversation SET updated_at=updated_at WHERE id=? AND agent_type=? AND external_id=? AND deleted_at IS NULL",vec![cid.into(),agent_key(agent)?.into(),external.into()])).await?;
    if current.rows_affected() != 1 {
        return Ok(false);
    }
    persist_turns_with_refs(&tx, cid, sid, turns, refs).await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn pending_request(
    conn: &DatabaseConnection,
    cid: i32,
) -> Result<Option<AgentSwitchRequest>, DbError> {
    let row=conn.query_one(stmt("SELECT r.request_json FROM business_agent_switch r JOIN business_conversation_state b ON b.pending_request_id=r.request_id WHERE b.conversation_id=? AND b.enabled=1 AND r.status IN ('queued','preparing')",vec![cid.into()])).await?;
    row.map(|r| decode(&r.try_get::<String>("", "request_json")?))
        .transpose()
}

pub async fn set_switch_status(
    conn: &DatabaseConnection,
    request: &AgentSwitchRequest,
    status: &str,
    error: Option<String>,
) -> Result<(), DbError> {
    let tx = conn.begin().await?;
    let result=tx.execute(stmt("UPDATE business_agent_switch SET status=?,error=?,updated_at=CURRENT_TIMESTAMP WHERE request_id=? AND status IN ('queued','preparing') AND status<>?",vec![status.into(),error.into(),request.request_id.clone().into(),status.into()])).await?;
    if result.rows_affected() > 0 {
        tx.execute(stmt("UPDATE business_conversation_state SET revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE conversation_id=? AND pending_request_id=?",vec![request.conversation_id.into(),request.request_id.clone().into()])).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn set_switch_phase(
    conn: &DatabaseConnection,
    request: &AgentSwitchRequest,
    phase: &str,
) -> Result<(), DbError> {
    let tx = conn.begin().await?;
    let changed = tx.execute(stmt("UPDATE business_agent_switch SET phase=?,updated_at=CURRENT_TIMESTAMP WHERE request_id=? AND status='preparing' AND (phase IS NULL OR phase<>?)", vec![phase.into(), request.request_id.clone().into(), phase.into()])).await?;
    if changed.rows_affected() > 0 {
        tx.execute(stmt("UPDATE business_conversation_state SET revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE conversation_id=? AND pending_request_id=?",vec![request.conversation_id.into(),request.request_id.clone().into()])).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn save_rule_context(
    conn: &DatabaseConnection,
    cid: i32,
    agent: AgentType,
    context: &str,
) -> Result<(), DbError> {
    // Only one snapshot per engine, never a chain of handoff summaries.
    conn.execute(stmt("UPDATE business_conversation_state SET rule_context=json_set(rule_context,?,?) WHERE conversation_id=?",vec![format!("$.{}", agent_key(agent)?).into(),context.into(),cid.into()])).await?;
    Ok(())
}

pub async fn rule_context(
    conn: &DatabaseConnection,
    cid: i32,
) -> Result<BTreeMap<String, String>, DbError> {
    let row = conn
        .query_one(stmt(
            "SELECT rule_context FROM business_conversation_state WHERE conversation_id=?",
            vec![cid.into()],
        ))
        .await?;
    row.map(|r| decode(&r.try_get::<String>("", "rule_context")?))
        .transpose()
        .map(|r| r.unwrap_or_default())
}

pub async fn activate(
    conn: &DatabaseConnection,
    request: &AgentSwitchRequest,
    external: &str,
) -> Result<(), DbError> {
    let tx = conn.begin().await?;
    let changed=tx.execute(stmt("UPDATE business_conversation_state SET revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE conversation_id=? AND pending_request_id=? AND enabled=1 AND EXISTS(SELECT 1 FROM business_agent_switch r WHERE r.request_id=pending_request_id AND r.status IN ('queued','preparing'))",vec![request.conversation_id.into(),request.request_id.clone().into()])).await?;
    if changed.rows_affected() != 1 {
        let succeeded = tx.query_one(stmt("SELECT c.id FROM conversation c JOIN business_agent_switch r ON r.conversation_id=c.id JOIN business_conversation_state b ON b.conversation_id=c.id WHERE c.id=? AND c.agent_type=? AND c.external_id=? AND b.pending_request_id=r.request_id AND r.request_id=? AND r.status='succeeded' AND r.request_json=?",vec![request.conversation_id.into(),agent_key(request.target_agent)?.into(),external.into(),request.request_id.clone().into(),json(request)?.into()])).await?.is_some();
        if succeeded {
            return Ok(());
        }
        return Err(DbError::Conflict("Switch request was superseded".into()));
    }
    tx.execute(stmt("INSERT INTO conversation_agent_session(conversation_id,agent_type,external_id,mode_id,config_values) VALUES(?,?,?,?,?) ON CONFLICT(conversation_id,agent_type) DO NOTHING",vec![request.conversation_id.into(),agent_key(request.target_agent)?.into(),external.into(),request.preferred_mode_id.clone().into(),json(&request.preferred_config_values)?.into()])).await?;
    let registered:String=tx.query_one(stmt("SELECT external_id FROM conversation_agent_session WHERE conversation_id=? AND agent_type=?",vec![request.conversation_id.into(),agent_key(request.target_agent)?.into()])).await?.ok_or_else(||DbError::Conflict("Target session was not registered".into()))?.try_get("","external_id")?;
    if registered != external {
        return Err(DbError::Conflict("Target session identity changed".into()));
    }
    tx.execute(stmt(
        "UPDATE conversation SET agent_type=?,external_id=?,model=NULL WHERE id=?",
        vec![
            agent_key(request.target_agent)?.into(),
            external.into(),
            request.conversation_id.into(),
        ],
    ))
    .await?;
    tx.execute(stmt("UPDATE business_agent_switch SET status='succeeded',error=NULL,updated_at=CURRENT_TIMESTAMP WHERE request_id=?",vec![request.request_id.clone().into()])).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn history(
    conn: &DatabaseConnection,
    cid: i32,
    before: Option<i64>,
    user_limit: usize,
) -> Result<(Vec<MessageTurn>, Option<i64>), DbError> {
    let (turns, next, _) = history_with_output_refs(conn, cid, before, user_limit).await?;
    Ok((turns, next))
}

pub async fn history_with_output_refs(
    conn: &DatabaseConnection,
    cid: i32,
    before: Option<i64>,
    user_limit: usize,
) -> Result<
    (
        Vec<MessageTurn>,
        Option<i64>,
        Vec<ConversationOutputTurnRef>,
    ),
    DbError,
> {
    let boundary = if user_limit == usize::MAX {
        None
    } else {
        conn.query_one(stmt("SELECT sequence FROM business_message WHERE conversation_id=? AND role='user' AND (? IS NULL OR sequence<?) ORDER BY sequence DESC LIMIT 1 OFFSET ?",vec![cid.into(),before.into(),before.into(),(user_limit.clamp(1,100).saturating_sub(1) as i64).into()])).await?
    };
    let start = boundary
        .map(|r| r.try_get::<i64>("", "sequence"))
        .transpose()?
        .unwrap_or(0);
    let rows=conn.query_all(stmt("SELECT sequence,session_id,native_turn_id,turn_json FROM business_message WHERE conversation_id=? AND sequence>=? AND (? IS NULL OR sequence<?) ORDER BY sequence",vec![cid.into(),start.into(),before.into(),before.into()])).await?;
    let mut turns = Vec::with_capacity(rows.len());
    let mut refs = Vec::with_capacity(rows.len());
    for row in rows {
        let stored: StoredBusinessTurn = decode(&row.try_get::<String>("", "turn_json")?)?;
        let mut turn = stored.turn;
        let sid: i64 = row.try_get("", "session_id")?;
        turn.id = format!(
            "business:{sid}:{}",
            row.try_get::<String>("", "native_turn_id")?
        );
        let reference = ConversationOutputTurnRef {
            id: turn.id.clone(),
            timestamp: turn.timestamp,
            is_user: matches!(turn.role, crate::models::TurnRole::User),
            prompt_fingerprint: stored
                .codeg_prompt_fingerprint
                .or_else(|| crate::artifact_tracker::user_turn_fingerprint(&turn)),
        };
        refs.push(reference);
        turns.push(turn);
    }
    let more = if start > 0 {
        conn.query_one(stmt(
            "SELECT sequence FROM business_message WHERE conversation_id=? AND sequence<? LIMIT 1",
            vec![cid.into(), start.into()],
        ))
        .await?
        .is_some()
    } else {
        false
    };
    Ok((turns, more.then_some(start), refs))
}

pub async fn update_task_state(
    conn: &DatabaseConnection,
    cid: i32,
    revision: i64,
    task: &BusinessTaskState,
) -> Result<(), DbError> {
    let payload = json(task)?;
    if payload.len() > 32768 {
        return Err(DbError::Validation("Task state exceeds32KiB".into()));
    }
    let result=conn.execute(stmt("UPDATE business_conversation_state SET task_state=?,revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE conversation_id=? AND revision=? AND enabled=1",vec![payload.into(),cid.into(),revision.into()])).await?;
    if result.rows_affected() != 1 {
        return Err(DbError::Conflict(
            "Task state changed; reload before saving".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedMemory {
    pub scope: String,
    pub scope_id: i32,
    pub name: String,
    pub content: String,
    pub source: String,
    pub revision: i64,
    pub updated_at: String,
}

pub async fn shared_memory(
    conn: &DatabaseConnection,
    cid: i32,
    folder_id: i32,
) -> Result<Vec<SharedMemory>, DbError> {
    conn.query_all(stmt("SELECT * FROM business_shared_memory WHERE (scope='global' AND scope_id=0) OR (scope='project' AND scope_id=?) OR (scope='task' AND scope_id=?) ORDER BY scope,name",vec![folder_id.into(),cid.into()])).await?.into_iter().map(|r|Ok(SharedMemory {scope:r.try_get("","scope")?,scope_id:r.try_get("","scope_id")?,name:r.try_get("","name")?,content:r.try_get("","content")?,source:r.try_get("","source")?,revision:r.try_get("","revision")?,updated_at:r.try_get("","updated_at")?})).collect()
}

pub async fn save_memory(conn: &DatabaseConnection, item: &SharedMemory) -> Result<(), DbError> {
    if !matches!(item.scope.as_str(), "global" | "project" | "task")
        || item.content.len() > 16384
        || item.name.is_empty()
        || item.name.len() > 128
        || item.source != "user"
        || item.scope_id < 0
        || item.scope == "global" && item.scope_id != 0
    {
        return Err(DbError::Validation(
            "Invalid shared memory; only explicit user facts are accepted".into(),
        ));
    }
    let result=conn.execute(stmt("INSERT INTO business_shared_memory(scope,scope_id,name,content,source,revision) SELECT ?,?,?,?,?,1 WHERE ?=0 ON CONFLICT(scope,scope_id,name) DO NOTHING",vec![item.scope.clone().into(),item.scope_id.into(),item.name.clone().into(),item.content.clone().into(),item.source.clone().into(),item.revision.into()])).await?;
    if result.rows_affected() == 1 {
        return Ok(());
    }
    let result=conn.execute(stmt("UPDATE business_shared_memory SET content=?,source='user',revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE scope=? AND scope_id=? AND name=? AND revision=?",vec![item.content.clone().into(),item.scope.clone().into(),item.scope_id.into(),item.name.clone().into(),item.revision.into()])).await?;
    if result.rows_affected() != 1 {
        return Err(DbError::Conflict(
            "Shared memory changed; reload before saving".into(),
        ));
    }
    Ok(())
}

pub fn digest_blocks(blocks: &[crate::acp::types::PromptInputBlock]) -> Result<String, DbError> {
    let mut normalized = blocks.to_vec();
    for block in &mut normalized {
        if let crate::acp::types::PromptInputBlock::Image {
            data,
            uri: Some(uri),
            ..
        } = block
        {
            if uri.starts_with("file://") {
                data.clear();
            }
        }
    }
    Ok(format!(
        "{:x}",
        Sha256::digest(json(&normalized)?.as_bytes())
    ))
}

/// Atomic durable admission. An unresolved reservation is deliberately NOT
/// replayed after a crash: no ACP protocol can atomically commit SQLite and an
/// arbitrary engine's prompt. The retained draft stays available for recovery.
pub async fn reserve_prompt(
    conn: &DatabaseConnection,
    cid: i32,
    id: &str,
    sid: i64,
    blocks: &[crate::acp::types::PromptInputBlock],
    context_sequence: i64,
) -> Result<bool, DbError> {
    let digest = digest_blocks(blocks)?;
    let result=conn.execute(stmt("INSERT INTO business_prompt_receipt(conversation_id,message_id,session_id,body_digest,original_blocks,context_sequence,status) VALUES(?,?,?,?,?,?,'reserved') ON CONFLICT(conversation_id,message_id) DO NOTHING",vec![cid.into(),id.into(),sid.into(),digest.clone().into(),json(&blocks)?.into(),context_sequence.into()])).await?;
    if result.rows_affected() == 1 {
        return Ok(true);
    }
    let row=conn.query_one(stmt("SELECT body_digest,status FROM business_prompt_receipt WHERE conversation_id=? AND message_id=?",vec![cid.into(),id.into()])).await?.unwrap();
    if row.try_get::<String>("", "body_digest")? != digest {
        return Err(DbError::Conflict(
            "Message ID reused for different content".into(),
        ));
    }
    if row.try_get::<String>("", "status")? == "accepted" {
        return Ok(false);
    }
    Err(DbError::Conflict("Prompt delivery is unresolved. The original draft is retained; reconcile it before retrying.".into()))
}

pub async fn accept_prompt(
    conn: &DatabaseConnection,
    cid: i32,
    id: &str,
    sid: i64,
    sequence: i64,
    bridge_boundary: i64,
) -> Result<(), DbError> {
    let tx = conn.begin().await?;
    tx.execute(stmt("UPDATE business_prompt_receipt SET status='accepted' WHERE conversation_id=? AND message_id=? AND session_id=?",vec![cid.into(),id.into(),sid.into()])).await?;
    tx.execute(stmt("UPDATE conversation_agent_session SET seen_sequence=MAX(seen_sequence,?),last_bridge_sequence=MAX(last_bridge_sequence,?) WHERE id=? AND conversation_id=?",vec![sequence.into(),bridge_boundary.into(),sid.into(),cid.into()])).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn release_unsent_prompt(
    conn: &DatabaseConnection,
    cid: i32,
    id: &str,
) -> Result<(), DbError> {
    conn.execute(stmt("DELETE FROM business_prompt_receipt WHERE conversation_id=? AND message_id=? AND status='reserved'",vec![cid.into(),id.into()])).await?;
    Ok(())
}

pub async fn pending_conversations(conn: &DatabaseConnection) -> Result<Vec<i32>, DbError> {
    conn.query_all(stmt("SELECT DISTINCT b.conversation_id FROM business_conversation_state b JOIN business_agent_switch r ON r.request_id=b.pending_request_id WHERE b.enabled=1 AND r.status IN ('queued','preparing')",vec![])).await?.into_iter().map(|r|r.try_get("","conversation_id").map_err(DbError::from)).collect()
}

pub async fn mark_own_history_seen(
    conn: &DatabaseConnection,
    sid: i64,
    sequence: i64,
) -> Result<(), DbError> {
    conn.execute(stmt(
        "UPDATE conversation_agent_session SET seen_sequence=MAX(seen_sequence,?) WHERE id=?",
        vec![sequence.into(), sid.into()],
    ))
    .await?;
    Ok(())
}

pub async fn context_increment(
    conn: &DatabaseConnection,
    cid: i32,
    sid: i64,
    after: i64,
    through: i64,
) -> Result<Vec<(i64, MessageTurn)>, DbError> {
    let rows=conn.query_all(stmt("SELECT sequence,turn_json FROM business_message WHERE conversation_id=? AND session_id<>? AND sequence>? AND sequence<=? ORDER BY sequence LIMIT 129",vec![cid.into(),sid.into(),after.into(),through.into()])).await?;
    rows.into_iter()
        .map(|r| {
            Ok((
                r.try_get("", "sequence")?,
                decode(&r.try_get::<String>("", "turn_json")?)?,
            ))
        })
        .collect()
}

pub async fn has_unseen_context(
    conn: &DatabaseConnection,
    cid: i32,
    sid: i64,
    after: i64,
) -> Result<bool, DbError> {
    Ok(conn.query_one(stmt("SELECT sequence FROM business_message WHERE conversation_id=? AND session_id<>? AND sequence>? AND role IN ('user','assistant') LIMIT 1",vec![cid.into(),sid.into(),after.into()])).await?.is_some())
}

/// A small recent window accompanies a budget-limited chronological bridge.
/// It does not acknowledge the older gap; the contiguous cursor still advances
/// only through records actually delivered by `context_increment`.
pub async fn recent_context_increment(
    conn: &DatabaseConnection,
    cid: i32,
    sid: i64,
    after: i64,
    through: i64,
) -> Result<Vec<(i64, MessageTurn)>, DbError> {
    let rows = conn.query_all(stmt("SELECT sequence,turn_json FROM business_message WHERE conversation_id=? AND session_id<>? AND sequence>? AND sequence<=? ORDER BY sequence DESC LIMIT 8",vec![cid.into(),sid.into(),after.into(),through.into()])).await?;
    rows.into_iter()
        .rev()
        .map(|r| {
            Ok((
                r.try_get("", "sequence")?,
                decode(&r.try_get::<String>("", "turn_json")?)?,
            ))
        })
        .collect()
}

pub fn now() -> String {
    Utc::now().to_rfc3339()
}

pub async fn save_session_settings(
    conn: &DatabaseConnection,
    cid: i32,
    agent: AgentType,
    mode: Option<String>,
    config: &BTreeMap<String, String>,
) -> Result<(), DbError> {
    conn.execute(stmt("UPDATE conversation_agent_session SET mode_id=?,config_values=? WHERE conversation_id=? AND agent_type=? AND EXISTS(SELECT 1 FROM conversation c WHERE c.id=conversation_id AND c.agent_type=conversation_agent_session.agent_type AND c.external_id=conversation_agent_session.external_id)",vec![mode.into(),json(config)?.into(),cid.into(),agent_key(agent)?.into()])).await?;
    Ok(())
}

pub async fn find_session(
    conn: &DatabaseConnection,
    cid: i32,
    agent: AgentType,
) -> Result<Option<BusinessAgentSession>, DbError> {
    conn.query_one(stmt(
        "SELECT * FROM conversation_agent_session WHERE conversation_id=? AND agent_type=?",
        vec![cid.into(), agent_key(agent)?.into()],
    ))
    .await?
    .map(session)
    .transpose()
}

pub async fn prompt_is_accepted(
    conn: &DatabaseConnection,
    cid: i32,
    id: &str,
) -> Result<bool, DbError> {
    Ok(conn.query_one(stmt("SELECT message_id FROM business_prompt_receipt WHERE conversation_id=? AND message_id=? AND status='accepted'",vec![cid.into(),id.into()])).await?.is_some())
}

/// Import the original bounded pages in chronological order. Only the final
/// transaction enables canonical history, so readers never see half an import.
pub async fn initialize_archive(
    conn: &DatabaseConnection,
    summary: &DbConversationSummary,
    pages: &[std::path::PathBuf],
) -> Result<(), DbError> {
    if is_managed(conn, summary.id).await? {
        return Ok(());
    }
    let tx = conn.begin().await?;
    let external = summary
        .external_id
        .clone()
        .ok_or_else(|| DbError::Validation("Original session is missing".into()))?;
    tx.execute(stmt("INSERT INTO business_conversation_state(conversation_id,original_agent,original_external_id,original_model) VALUES(?,?,?,?) ON CONFLICT(conversation_id) DO NOTHING",vec![summary.id.into(),agent_key(summary.agent_type)?.into(),external.clone().into(),summary.model.clone().into()])).await?;
    tx.execute(stmt("INSERT INTO conversation_agent_session(conversation_id,agent_type,external_id) VALUES(?,?,?) ON CONFLICT(conversation_id,agent_type) DO NOTHING",vec![summary.id.into(),agent_key(summary.agent_type)?.into(),external.into()])).await?;
    let sid: i64 = tx
        .query_one(stmt(
            "SELECT id FROM conversation_agent_session WHERE conversation_id=? AND agent_type=?",
            vec![summary.id.into(), agent_key(summary.agent_type)?.into()],
        ))
        .await?
        .unwrap()
        .try_get("", "id")?;
    for path in pages.iter().rev() {
        let bytes = tokio::fs::read(path).await?;
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ArchivePage {
            WithRefs(BusinessHistoryArchivePage),
            Legacy(Vec<MessageTurn>),
        }
        let page: ArchivePage =
            serde_json::from_slice(&bytes).map_err(|e| DbError::Validation(e.to_string()))?;
        match page {
            ArchivePage::WithRefs(page) => {
                persist_turns_with_refs(&tx, summary.id, sid, &page.turns, &page.output_turn_refs)
                    .await?
            }
            ArchivePage::Legacy(turns) => persist_turns(&tx, summary.id, sid, &turns).await?,
        }
    }
    tx.execute(stmt(
        "UPDATE business_conversation_state SET enabled=1 WHERE conversation_id=?",
        vec![summary.id.into()],
    ))
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn session_owner(
    conn: &DatabaseConnection,
    agent: AgentType,
    external: &str,
) -> Result<Option<i32>, DbError> {
    let key = agent_key(agent)?;
    conn.query_one(stmt("SELECT s.conversation_id FROM conversation_agent_session s JOIN business_conversation_state b ON b.conversation_id=s.conversation_id WHERE s.agent_type=? AND s.external_id=? AND b.enabled=1 UNION SELECT s.conversation_id FROM business_agent_session_alias a JOIN conversation_agent_session s ON s.id=a.session_id JOIN business_conversation_state b ON b.conversation_id=s.conversation_id WHERE a.agent_type=? AND a.external_id=? AND b.enabled=1 LIMIT 1",vec![key.clone().into(),external.into(),key.into(),external.into()])).await?.map(|r|r.try_get("","conversation_id").map_err(DbError::from)).transpose()
}

/// Native rollover remains inside one engine. Preserve every old transcript
/// alias for lazy attachments, without creating a second visible conversation.
pub async fn bind_managed_external(
    conn: &DatabaseConnection,
    cid: i32,
    external: &str,
) -> Result<bool, DbError> {
    let Some(state) = state(conn, cid).await? else {
        return Ok(false);
    };
    let current = state
        .sessions
        .iter()
        .find(|s| s.agent_type == state.active_agent)
        .ok_or_else(|| DbError::Validation("Active business session is missing".into()))?;
    bind_managed_external_if_current(
        conn,
        cid,
        state.active_agent,
        &current.external_id,
        external,
    )
    .await?;
    Ok(true)
}

/// A history read can outlive a handoff. Its rollover discovery must never
/// rebind the newly active engine to the outgoing engine's transcript.
pub async fn bind_managed_external_if_current(
    conn: &DatabaseConnection,
    cid: i32,
    agent: AgentType,
    expected_external: &str,
    external: &str,
) -> Result<(), DbError> {
    if session_owner(conn, agent, external)
        .await?
        .is_some_and(|owner| owner != cid)
    {
        return Err(DbError::Conflict(
            "Native session belongs to another business".into(),
        ));
    }
    let key = agent_key(agent)?;
    let tx = conn.begin().await?;
    let changed = tx.execute(stmt("UPDATE conversation SET external_id=? WHERE id=? AND agent_type=? AND external_id=? AND deleted_at IS NULL AND EXISTS(SELECT 1 FROM business_conversation_state b WHERE b.conversation_id=conversation.id AND b.enabled=1)",vec![external.into(),cid.into(),key.clone().into(),expected_external.into()])).await?;
    if changed.rows_affected() != 1 {
        return Err(DbError::Conflict(
            "Business session changed during history read".into(),
        ));
    }
    if expected_external == external {
        tx.commit().await?;
        return Ok(());
    }
    tx.execute(stmt("INSERT INTO business_agent_session_alias(agent_type,external_id,session_id) SELECT agent_type,external_id,id FROM conversation_agent_session WHERE conversation_id=? AND agent_type=? ON CONFLICT(agent_type,external_id) DO NOTHING",vec![cid.into(),key.clone().into()])).await?;
    tx.execute(stmt("UPDATE conversation_agent_session SET external_id=? WHERE conversation_id=? AND agent_type=?",vec![external.into(),cid.into(),key.clone().into()])).await?;
    tx.execute(stmt("INSERT INTO business_agent_session_alias(agent_type,external_id,session_id) SELECT agent_type,external_id,id FROM conversation_agent_session WHERE conversation_id=? AND agent_type=? ON CONFLICT(agent_type,external_id) DO NOTHING",vec![cid.into(),key.into()])).await?;
    tx.execute(stmt("UPDATE business_conversation_state SET revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE conversation_id=?",vec![cid.into()])).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn verify_prompt_body(
    conn: &DatabaseConnection,
    cid: i32,
    id: &str,
    blocks: &[crate::acp::types::PromptInputBlock],
) -> Result<(), DbError> {
    if let Some(row)=conn.query_one(stmt("SELECT body_digest FROM business_prompt_receipt WHERE conversation_id=? AND message_id=?",vec![cid.into(),id.into()])).await? {
        if row.try_get::<String>("","body_digest")?!=digest_blocks(blocks)?{return Err(DbError::Conflict("Message ID reused for different content".into()))}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::service::conversation_service;
    use crate::db::test_helpers::{
        fresh_disk_db, fresh_in_memory_db, seed_conversation, seed_folder,
    };
    use crate::models::message::TurnRole;

    fn turn(id: &str, role: TurnRole, text: &str, duration: Option<u64>) -> MessageTurn {
        MessageTurn {
            id: id.into(),
            role,
            blocks: vec![crate::models::ContentBlock::Text { text: text.into() }],
            timestamp: Utc::now(),
            usage: None,
            duration_ms: duration,
            model: None,
            completed_at: duration.map(|_| Utc::now()),
            agent_message_id: None,
            native_task_id: None,
        }
    }
    async fn setup() -> (crate::db::AppDatabase, i32) {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/isolated-business-test").await;
        let cid = seed_conversation(&db, folder, AgentType::Codex).await;
        conversation_service::bind_external_id(&db.conn, cid, "codex-original", &[])
            .await
            .unwrap();
        let summary = conversation_service::get_by_id(&db.conn, cid)
            .await
            .unwrap();
        initialize(
            &db.conn,
            &summary,
            &[
                turn("u1", TurnRole::User, "prepare conference", None),
                turn("a1", TurnRole::Assistant, "draft ready", Some(168795)),
            ],
        )
        .await
        .unwrap();
        (db, cid)
    }
    fn request(cid: i32, id: &str, target: AgentType, revision: Option<i64>) -> AgentSwitchRequest {
        AgentSwitchRequest {
            conversation_id: cid,
            request_id: id.into(),
            target_agent: target,
            expected_revision: revision,
            preferred_mode_id: None,
            preferred_config_values: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn business_output_identity_survives_deferred_images_handoff_and_partial_reparse() {
        let (db, cid) = setup().await;
        let session = find_session(&db.conn, cid, AgentType::Codex)
            .await
            .unwrap()
            .unwrap();
        let mut image_turn = turn("image-user", TurnRole::User, "describe attachment", None);
        image_turn.blocks.push(crate::models::ContentBlock::Image {
            data: "aGVsbG8=".into(),
            mime_type: "image/png".into(),
            uri: None,
        });
        let original = ConversationOutputTurnRef::from(&image_turn);
        let crate::models::ContentBlock::Image { data, uri, .. } = &mut image_turn.blocks[1] else {
            unreachable!()
        };
        data.clear();
        *uri = Some("codeg-history-content:opaque-original-image".into());
        assert_eq!(
            crate::artifact_tracker::user_turn_fingerprint(&image_turn),
            None
        );
        assert!(persist_current_tail(
            &db.conn,
            cid,
            session.id,
            AgentType::Codex,
            "codex-original",
            std::slice::from_ref(&image_turn),
            std::slice::from_ref(&original)
        )
        .await
        .unwrap());
        // Older/partial parsers may not supply refs; they must not replace the
        // original hash with the empty image preview's hash.
        persist_turns(&db.conn, cid, session.id, &[image_turn])
            .await
            .unwrap();
        let switch = request(cid, "image-handoff", AgentType::Pi, Some(0));
        queue_switch(&db.conn, &switch).await.unwrap();
        activate(&db.conn, &switch, "pi-image-peer").await.unwrap();
        let (turns, _, refs) = history_with_output_refs(&db.conn, cid, None, 25)
            .await
            .unwrap();
        let last = turns.last().unwrap();
        assert!(last.id.starts_with("business:"));
        assert_eq!(refs.last().unwrap().id, last.id);
        assert_eq!(
            refs.last().unwrap().prompt_fingerprint,
            original.prompt_fingerprint
        );
        assert!(
            matches!(&last.blocks[1], crate::models::ContentBlock::Image { data, .. } if data.is_empty()),
            "identity retention must not rehydrate large attachments"
        );
    }
    #[tokio::test]
    async fn pi_origin_retains_native_identity_settings_and_history_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::init_database(dir.path(), "0.40.0")
            .await
            .unwrap();
        let folder = seed_folder(&db, "/isolated-pi-business").await;
        let cid = seed_conversation(&db, folder, AgentType::Pi).await;
        conversation_service::bind_external_id(&db.conn, cid, "pi-original", &[])
            .await
            .unwrap();
        let summary = conversation_service::get_by_id(&db.conn, cid)
            .await
            .unwrap();
        initialize(
            &db.conn,
            &summary,
            &[
                turn("u1", TurnRole::User, "Pi original business", None),
                turn(
                    "a1",
                    TurnRole::Assistant,
                    "Pi original answer",
                    Some(168795),
                ),
            ],
        )
        .await
        .unwrap();
        let pi_config = BTreeMap::from([
            ("model".into(), "pi-provider/pi-model".into()),
            ("thought_level".into(), "high".into()),
        ]);
        save_session_settings(
            &db.conn,
            cid,
            AgentType::Pi,
            Some("high".into()),
            &pi_config,
        )
        .await
        .unwrap();
        for (agent, external) in [
            (AgentType::Codex, "codex-independent"),
            (AgentType::ClaudeCode, "claude-independent"),
            (AgentType::Pi, "pi-original"),
        ] {
            let revision = state(&db.conn, cid).await.unwrap().unwrap().revision;
            let req = request(cid, external, agent, Some(revision));
            queue_switch(&db.conn, &req).await.unwrap();
            activate(&db.conn, &req, external).await.unwrap();
        }
        db.conn.close().await.unwrap();
        let db = crate::db::init_database(dir.path(), "0.40.0")
            .await
            .unwrap();
        let restored = state(&db.conn, cid).await.unwrap().unwrap();
        assert_eq!(restored.active_agent, AgentType::Pi);
        assert_eq!(restored.sessions.len(), 3);
        let pi = find_session(&db.conn, cid, AgentType::Pi)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pi.external_id, "pi-original");
        assert_eq!(pi.mode_id.as_deref(), Some("high"));
        assert_eq!(pi.config_values, pi_config);
        let native = conversation_service::get_by_id(&db.conn, cid)
            .await
            .unwrap();
        assert_eq!(native.external_id.as_deref(), Some("pi-original"));
        let history = history(&db.conn, cid, None, 25).await.unwrap().0;
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].duration_ms, Some(168795));
        db.conn.close().await.unwrap();
    }

    #[tokio::test]
    async fn switches_preserve_business_and_engine_identities_with_incremental_cursors() {
        let (db, cid) = setup().await;
        let first = request(cid, "one", AgentType::ClaudeCode, Some(0));
        queue_switch(&db.conn, &first).await.unwrap();
        queue_switch(&db.conn, &first).await.unwrap();
        assert_eq!(state(&db.conn, cid).await.unwrap().unwrap().revision, 1);
        activate(&db.conn, &first, "claude-independent")
            .await
            .unwrap();
        let claude = find_session(&db.conn, cid, AgentType::ClaudeCode)
            .await
            .unwrap()
            .unwrap();
        let unseen = context_increment(&db.conn, cid, claude.id, 0, i64::MAX)
            .await
            .unwrap();
        assert_eq!(unseen.len(), 2);
        accept_prompt(
            &db.conn,
            cid,
            "no-receipt",
            claude.id,
            unseen.last().unwrap().0,
            unseen.last().unwrap().0,
        )
        .await
        .unwrap();
        assert!(
            context_increment(&db.conn, cid, claude.id, unseen.last().unwrap().0, i64::MAX)
                .await
                .unwrap()
                .is_empty()
        );
        persist_turns(
            &db.conn,
            cid,
            claude.id,
            &[
                turn("u1", TurnRole::User, "continue", None),
                turn("a1", TurnRole::Assistant, "claude answer", Some(615361)),
            ],
        )
        .await
        .unwrap();
        let return_request = request(cid, "two", AgentType::Codex, Some(2));
        queue_switch(&db.conn, &return_request).await.unwrap();
        activate(&db.conn, &return_request, "codex-original")
            .await
            .unwrap();
        let current = conversation_service::get_by_id(&db.conn, cid)
            .await
            .unwrap();
        assert_eq!(current.agent_type, AgentType::Codex);
        assert_eq!(current.external_id.as_deref(), Some("codex-original"));
        let state = state(&db.conn, cid).await.unwrap().unwrap();
        assert_eq!(state.sessions.len(), 2);
        let (history, _) = history(&db.conn, cid, None, 25).await.unwrap();
        assert_eq!(history.len(), 4);
        assert_ne!(history[0].id, history[2].id);
        assert_eq!(history[1].duration_ms, Some(168795));
        assert_eq!(history[3].duration_ms, Some(615361));
        assert_eq!(
            context_increment(&db.conn, cid, state.sessions[0].id, 2, i64::MAX)
                .await
                .unwrap()
                .len(),
            2
        );
    }
    #[tokio::test]
    async fn stale_clients_and_superseded_requests_cannot_publish() {
        let (db, cid) = setup().await;
        let one = request(cid, "one", AgentType::ClaudeCode, Some(0));
        queue_switch(&db.conn, &one).await.unwrap();
        assert!(queue_switch(
            &db.conn,
            &request(cid, "stale", AgentType::ClaudeCode, Some(0))
        )
        .await
        .is_err());
        let two = request(cid, "two", AgentType::Codex, Some(1));
        queue_switch(&db.conn, &two).await.unwrap();
        assert!(activate(&db.conn, &one, "wrong-old-session").await.is_err());
        assert_eq!(
            pending_request(&db.conn, cid)
                .await
                .unwrap()
                .unwrap()
                .request_id,
            "two"
        );
        let changed = request(cid, "two", AgentType::ClaudeCode, Some(1));
        assert!(queue_switch(&db.conn, &changed).await.is_err());
    }
    #[tokio::test]
    async fn failed_prepare_preserves_original_and_other_business() {
        let (db, cid) = setup().await;
        let folder = seed_folder(&db, "/other-test").await;
        let other = seed_conversation(&db, folder, AgentType::ClaudeCode).await;
        let req = request(cid, "failed", AgentType::ClaudeCode, Some(0));
        queue_switch(&db.conn, &req).await.unwrap();
        set_switch_status(&db.conn, &req, "failed", Some("auth unavailable".into()))
            .await
            .unwrap();
        assert!(pending_request(&db.conn, cid).await.unwrap().is_none());
        assert_eq!(
            conversation_service::get_by_id(&db.conn, cid)
                .await
                .unwrap()
                .external_id
                .as_deref(),
            Some("codex-original")
        );
        assert!(!is_managed(&db.conn, other).await.unwrap());
        assert_eq!(
            state(&db.conn, cid).await.unwrap().unwrap().sessions.len(),
            1
        );
    }
    #[tokio::test]
    async fn receipts_keep_original_draft_and_never_replay_ambiguous_delivery() {
        let (db, cid) = setup().await;
        let sid = state(&db.conn, cid).await.unwrap().unwrap().sessions[0].id;
        let blocks = vec![crate::acp::types::PromptInputBlock::Text {
            text: "once".into(),
        }];
        assert!(reserve_prompt(&db.conn, cid, "msg", sid, &blocks, 2)
            .await
            .unwrap());
        assert!(reserve_prompt(&db.conn, cid, "msg", sid, &blocks, 2)
            .await
            .is_err());
        accept_prompt(&db.conn, cid, "msg", sid, 2, 2)
            .await
            .unwrap();
        assert!(!reserve_prompt(&db.conn, cid, "msg", sid, &blocks, 2)
            .await
            .unwrap());
        assert!(reserve_prompt(&db.conn, cid, "msg", sid, &[], 2)
            .await
            .is_err());
        assert_eq!(
            state(&db.conn, cid).await.unwrap().unwrap().sessions[0].seen_sequence,
            2
        );
    }
    #[tokio::test]
    async fn paging_preserves_rounds_and_partial_reads_do_not_erase_native_timing() {
        let (db, cid) = setup().await;
        let sid = state(&db.conn, cid).await.unwrap().unwrap().sessions[0].id;
        persist_turns(
            &db.conn,
            cid,
            sid,
            &[
                turn("a1", TurnRole::Assistant, "draft ready", None),
                turn("u2", TurnRole::User, "next", None),
                turn("a2", TurnRole::Assistant, "next done", Some(309047)),
            ],
        )
        .await
        .unwrap();
        let (newest, next) = history(&db.conn, cid, None, 1).await.unwrap();
        assert_eq!(newest.len(), 2);
        let (older, stop) = history(&db.conn, cid, next, 1).await.unwrap();
        assert_eq!(older.len(), 2);
        assert_eq!(older[1].duration_ms, Some(168795));
        assert!(stop.is_none());
        assert_eq!(
            history(&db.conn, cid, None, usize::MAX)
                .await
                .unwrap()
                .0
                .len(),
            4
        );
    }
    #[tokio::test]
    async fn shared_user_memory_conflicts_and_task_updates_are_explicit() {
        let (db, cid) = setup().await;
        let item = SharedMemory {
            scope: "task".into(),
            scope_id: cid,
            name: "style".into(),
            content: "Use the existing official document template".into(),
            source: "user".into(),
            revision: 0,
            updated_at: String::new(),
        };
        save_memory(&db.conn, &item).await.unwrap();
        assert!(save_memory(&db.conn, &item).await.is_err());
        let mut unsafe_item = item.clone();
        unsafe_item.name = "inference".into();
        unsafe_item.source = "tool_error".into();
        assert!(save_memory(&db.conn, &unsafe_item).await.is_err());
        let task = BusinessTaskState {
            objective: "Conference".into(),
            ..Default::default()
        };
        update_task_state(&db.conn, cid, 0, &task).await.unwrap();
        assert!(update_task_state(&db.conn, cid, 0, &task).await.is_err());
        assert_eq!(
            state(&db.conn, cid)
                .await
                .unwrap()
                .unwrap()
                .task_state
                .objective,
            "Conference"
        );
    }
    #[tokio::test]
    async fn business_concurrent_clients_publish_once_and_retries_keep_the_revision() {
        let (db, cid) = setup().await;
        let a = request(cid, "client-a", AgentType::ClaudeCode, Some(0));
        let b = request(cid, "client-b", AgentType::ClaudeCode, Some(0));
        let (one, two) = tokio::join!(queue_switch(&db.conn, &a), queue_switch(&db.conn, &b));
        assert_ne!(one.is_ok(), two.is_ok());
        assert!(matches!(
            if one.is_ok() { &two } else { &one },
            Err(DbError::Conflict(_))
        ));
        let winner = if one.is_ok() { &a } else { &b };
        activate(&db.conn, winner, "claude-once").await.unwrap();
        let revision = state(&db.conn, cid).await.unwrap().unwrap().revision;
        queue_switch(&db.conn, winner).await.unwrap();
        activate(&db.conn, winner, "claude-once").await.unwrap();
        let current = state(&db.conn, cid).await.unwrap().unwrap();
        assert_eq!(current.revision, revision);
        assert_eq!(current.sessions.len(), 2);
        assert_eq!(current.active_agent, AgentType::ClaudeCode);
    }

    #[tokio::test]
    async fn business_switch_waits_for_a_concurrent_history_writer_without_a_stale_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(dir.path()).await;
        let folder = seed_folder(&db, &dir.path().to_string_lossy()).await;
        let cid = seed_conversation(&db, folder, AgentType::Codex).await;
        conversation_service::bind_external_id(&db.conn, cid, "codex-original", &[])
            .await
            .unwrap();
        let summary = conversation_service::get_by_id(&db.conn, cid)
            .await
            .unwrap();
        initialize(&db.conn, &summary, &[]).await.unwrap();

        // WAL permits reads while another connection writes. A read-first
        // switch would take that older snapshot, then fail immediately when
        // promoting it to a writer, rather than waiting for busy_timeout.
        let writer_conn = sea_orm::Database::connect(format!(
            "sqlite:{}?mode=rw",
            dir.path().join("source.db").to_string_lossy()
        ))
        .await
        .unwrap();
        let writer = writer_conn.begin().await.unwrap();
        writer
            .execute(stmt(
                "UPDATE conversation SET message_count=message_count+1 WHERE id=?",
                vec![cid.into()],
            ))
            .await
            .unwrap();
        let conn = db.conn.clone();
        let req = request(cid, "after-history-write", AgentType::ClaudeCode, Some(0));
        let mut pending = tokio::spawn(async move { queue_switch(&conn, &req).await });
        let early = tokio::time::timeout(std::time::Duration::from_millis(100), &mut pending).await;
        assert!(
            early.is_err(),
            "Switch did not wait for the writer: {early:?}"
        );
        writer.commit().await.unwrap();
        pending.await.unwrap().unwrap();

        let current = state(&db.conn, cid).await.unwrap().unwrap();
        assert_eq!(current.revision, 1);
        assert_eq!(current.active_agent, AgentType::Codex);
        assert_eq!(
            current.pending_request_id.as_deref(),
            Some("after-history-write")
        );
        queue_switch(
            &db.conn,
            &request(cid, "after-history-write", AgentType::ClaudeCode, Some(0)),
        )
        .await
        .unwrap();
        assert_eq!(state(&db.conn, cid).await.unwrap().unwrap().revision, 1);
        writer_conn.close().await.unwrap();
        db.conn.close().await.unwrap();
    }

    #[tokio::test]
    async fn business_rollover_keeps_original_history_and_stale_parses_cannot_rebind() {
        let (db, cid) = setup().await;
        let original = find_session(&db.conn, cid, AgentType::Codex)
            .await
            .unwrap()
            .unwrap();
        bind_managed_external_if_current(
            &db.conn,
            cid,
            AgentType::Codex,
            "codex-original",
            "codex-after-clear",
        )
        .await
        .unwrap();
        persist_turns(
            &db.conn,
            cid,
            original.id,
            &[
                turn("u1", TurnRole::User, "after clear", None),
                turn("a1", TurnRole::Assistant, "new answer", Some(309047)),
            ],
        )
        .await
        .unwrap();
        let turns = history(&db.conn, cid, None, 25).await.unwrap().0;
        assert_eq!(turns.len(), 4);
        assert_ne!(turns[0].id, turns[2].id);
        assert_eq!(turns[1].duration_ms, Some(168795));
        assert_eq!(turns[3].duration_ms, Some(309047));
        assert_eq!(
            session_owner(&db.conn, AgentType::Codex, "codex-original")
                .await
                .unwrap(),
            Some(cid)
        );
        let revision = state(&db.conn, cid).await.unwrap().unwrap().revision;
        let switch = request(cid, "after-rollover", AgentType::ClaudeCode, Some(revision));
        queue_switch(&db.conn, &switch).await.unwrap();
        activate(&db.conn, &switch, "claude-owned").await.unwrap();
        assert!(bind_managed_external_if_current(
            &db.conn,
            cid,
            AgentType::Codex,
            "codex-after-clear",
            "codex-late"
        )
        .await
        .is_err());
        assert!(!persist_current_tail(
            &db.conn,
            cid,
            original.id,
            AgentType::Codex,
            "codex-after-clear",
            &[turn("late", TurnRole::Assistant, "obsolete", None)],
            &[]
        )
        .await
        .unwrap());
        assert_eq!(
            conversation_service::get_by_id(&db.conn, cid)
                .await
                .unwrap()
                .external_id
                .as_deref(),
            Some("claude-owned")
        );
        assert_eq!(history(&db.conn, cid, None, 25).await.unwrap().0.len(), 4);
    }

    #[tokio::test]
    async fn business_restart_recovers_queued_handoff_cursor_memory_and_unsent_draft() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::init_database(dir.path(), "0.40.0")
            .await
            .unwrap();
        let folder = seed_folder(&db, "/isolated-restart-business").await;
        let cid = seed_conversation(&db, folder, AgentType::Codex).await;
        conversation_service::bind_external_id(&db.conn, cid, "restart-codex", &[])
            .await
            .unwrap();
        let summary = conversation_service::get_by_id(&db.conn, cid)
            .await
            .unwrap();
        initialize(
            &db.conn,
            &summary,
            &[turn("u", TurnRole::User, "original", None)],
        )
        .await
        .unwrap();
        let sid = find_session(&db.conn, cid, AgentType::Codex)
            .await
            .unwrap()
            .unwrap()
            .id;
        let req = request(cid, "recover-after-restart", AgentType::Pi, None);
        queue_switch(&db.conn, &req).await.unwrap();
        set_switch_status(&db.conn, &req, "preparing", None)
            .await
            .unwrap();
        set_switch_phase(&db.conn, &req, "verifying_target")
            .await
            .unwrap();
        save_rule_context(
            &db.conn,
            cid,
            AgentType::Codex,
            "Keep selected instance safety boundaries and ledger reading requirements",
        )
        .await
        .unwrap();
        let blocks = vec![crate::acp::types::PromptInputBlock::Text {
            text: "retained once".into(),
        }];
        reserve_prompt(&db.conn, cid, "draft", sid, &blocks, 1)
            .await
            .unwrap();
        save_memory(
            &db.conn,
            &SharedMemory {
                scope: "task".into(),
                scope_id: cid,
                name: "template".into(),
                content: "Keep template A".into(),
                source: "user".into(),
                revision: 0,
                updated_at: String::new(),
            },
        )
        .await
        .unwrap();
        db.conn.close().await.unwrap();
        let db = crate::db::init_database(dir.path(), "0.40.0")
            .await
            .unwrap();
        assert_eq!(pending_conversations(&db.conn).await.unwrap(), vec![cid]);
        let recovered = state(&db.conn, cid).await.unwrap().unwrap();
        assert_eq!(recovered.switch_status.as_deref(), Some("preparing"));
        assert_eq!(recovered.switch_phase.as_deref(), Some("verifying_target"));
        assert!(rule_context(&db.conn, cid).await.unwrap()["codex"].contains("ledger reading"));
        assert!(is_switch_retry(&db.conn, &req).await.unwrap());
        assert_eq!(
            pending_request(&db.conn, cid)
                .await
                .unwrap()
                .unwrap()
                .request_id,
            req.request_id
        );
        assert_eq!(
            shared_memory(&db.conn, cid, folder).await.unwrap()[0].content,
            "Keep template A"
        );
        assert!(
            reserve_prompt(&db.conn, cid, "draft", sid, &blocks, 1)
                .await
                .is_err(),
            "Ambiguous native delivery is never replayed on restart"
        );
        assert_eq!(history(&db.conn, cid, None, 25).await.unwrap().0.len(), 1);
        db.conn.close().await.unwrap();
    }

    #[tokio::test]
    async fn rollback_restores_legacy_projection_without_deleting_new_history() {
        use sea_orm_migration::{MigrationTrait, SchemaManager};
        let (db, cid) = setup().await;
        let req = request(cid, "switch", AgentType::ClaudeCode, Some(0));
        queue_switch(&db.conn, &req).await.unwrap();
        activate(&db.conn, &req, "claude-kept").await.unwrap();
        crate::db::migration::m20261025_000001_business_agent_sessions::Migration
            .down(&SchemaManager::new(&db.conn))
            .await
            .unwrap();
        let summary = conversation_service::get_by_id(&db.conn, cid)
            .await
            .unwrap();
        assert_eq!(summary.agent_type, AgentType::Codex);
        assert!(!is_managed(&db.conn, cid).await.unwrap());
        assert_eq!(history(&db.conn, cid, None, 25).await.unwrap().0.len(), 2);
        assert!(find_session(&db.conn, cid, AgentType::ClaudeCode)
            .await
            .unwrap()
            .is_some());
    }
}
