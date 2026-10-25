//! Durable external submissions. Target selection is immutable at acceptance.
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement, TransactionTrait,
    Value,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{acp::types::PromptInputBlock, db::error::DbError, models::agent::AgentType};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitRequest {
    #[serde(alias = "conversation_id")]
    pub conversation_id: i32,
    #[serde(alias = "client_message_id")]
    pub client_message_id: String,
    pub blocks: Vec<PromptInputBlock>,
    #[serde(default)]
    #[serde(alias = "required_agent")]
    pub required_agent: Option<AgentType>,
    #[serde(default = "queue_mode")]
    pub mode: String,
    #[serde(default = "default_deadline")]
    #[serde(alias = "deadline_seconds")]
    pub deadline_seconds: u32,
    #[serde(default)]
    pub metadata: std::collections::BTreeMap<String, String>,
}
fn queue_mode() -> String {
    "queue".into()
}
fn default_deadline() -> u32 {
    3600
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub conversation_id: i32,
    pub client_message_id: String,
    pub target_agent: AgentType,
    pub target_external_id: String,
    pub target_generation: String,
    pub submitted_revision: i64,
    pub target_settings: ExecutionSettings,
    pub mode: String,
    pub status: String,
    pub error: Option<String>,
    pub task_id: Option<String>,
    pub connection_id: Option<String>,
    pub deadline_at: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip)]
    pub request_json: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ExecutionSettings {
    pub mode: Option<String>,
    pub config: BTreeMap<String, String>,
}
impl ExecutionSettings {
    pub fn snapshot(state: &crate::acp::session_state::SessionState) -> Self {
        use crate::acp::types::SessionConfigKindInfo;
        Self {
            mode: state.modes.as_ref().map(|m| m.current_mode_id.clone()),
            config: state
                .config_options
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|o| {
                    (
                        o.id.clone(),
                        match &o.kind {
                            SessionConfigKindInfo::Select(v) => v.current_value.clone(),
                            SessionConfigKindInfo::Boolean(v) => v.current_value.to_string(),
                        },
                    )
                })
                .collect(),
        }
    }
    pub fn matches(&self, state: &crate::acp::session_state::SessionState) -> bool {
        self == &Self::snapshot(state)
    }
}
impl Delivery {
    pub fn request(&self) -> Result<SubmitRequest, DbError> {
        serde_json::from_str(&self.request_json).map_err(|e| DbError::Validation(e.to_string()))
    }
    pub fn terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "completed" | "failed" | "cancelled" | "uncertain" | "suspended"
        )
    }
}
fn stmt(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, sql, values)
}
fn row(row: QueryResult) -> Result<Delivery, DbError> {
    let key: String = row.try_get("", "target_agent")?;
    Ok(Delivery {
        id: row.try_get("", "id")?,
        conversation_id: row.try_get("", "conversation_id")?,
        client_message_id: row.try_get("", "client_message_id")?,
        target_agent: serde_json::from_str(&format!("\"{key}\""))
            .map_err(|e| DbError::Validation(e.to_string()))?,
        target_external_id: row.try_get("", "target_external_id")?,
        target_generation: row.try_get("", "target_generation")?,
        submitted_revision: row.try_get("", "submitted_revision")?,
        target_settings: serde_json::from_str(&row.try_get::<String>("", "target_settings")?)
            .map_err(|e| DbError::Validation(e.to_string()))?,
        mode: row.try_get("", "mode")?,
        status: row.try_get("", "status")?,
        error: row.try_get("", "error")?,
        task_id: row.try_get("", "task_id")?,
        connection_id: row.try_get("", "connection_id")?,
        deadline_at: row.try_get("", "deadline_at")?,
        created_at: row.try_get("", "created_at")?,
        updated_at: row.try_get("", "updated_at")?,
        request_json: row.try_get("", "request_json")?,
    })
}

const TARGET_SQL: &str = "SELECT c.agent_type,c.external_id,COALESCE(b.revision,0) AS revision,
 (SELECT r.status FROM business_agent_switch r WHERE r.request_id=b.pending_request_id) AS switch_status,
 COALESCE((SELECT request_id FROM business_agent_switch r WHERE r.conversation_id=c.id AND r.status='succeeded' ORDER BY rowid DESC LIMIT 1),'original') AS generation
 FROM conversation c LEFT JOIN business_conversation_state b ON b.conversation_id=c.id AND b.enabled=1
 WHERE c.id=? AND c.deleted_at IS NULL";

pub async fn submit(
    conn: &DatabaseConnection,
    request: &SubmitRequest,
) -> Result<Delivery, DbError> {
    submit_with_settings(conn, request, &ExecutionSettings::default()).await
}
pub async fn submit_with_settings(
    conn: &DatabaseConnection,
    request: &SubmitRequest,
    settings: &ExecutionSettings,
) -> Result<Delivery, DbError> {
    if request.client_message_id.trim().is_empty()
        || request.client_message_id.len() > 128
        || request.client_message_id.starts_with("codeg:")
        || request.blocks.is_empty()
        || !(1..=86400).contains(&request.deadline_seconds)
        || request.metadata.len() > 16
        || request
            .metadata
            .iter()
            .any(|(k, v)| k.len() > 128 || v.len() > 2048)
    {
        return Err(DbError::Validation("INVALID_BUSINESS_MESSAGE".into()));
    }
    if !matches!(request.mode.as_str(), "queue" | "steer") {
        return Err(DbError::Validation(
            "DELIVERY_MODE_UNSUPPORTED: use queue or steer".into(),
        ));
    }
    let payload = serde_json::to_string(request).map_err(|e| DbError::Validation(e.to_string()))?;
    let tx = conn.begin().await?;
    tx.execute(stmt(
        "UPDATE conversation SET updated_at=updated_at WHERE id=?",
        vec![request.conversation_id.into()],
    ))
    .await?;
    if let Some(old) = tx
        .query_one(stmt(
            "SELECT * FROM business_delivery WHERE conversation_id=? AND client_message_id=?",
            vec![
                request.conversation_id.into(),
                request.client_message_id.clone().into(),
            ],
        ))
        .await?
    {
        let old = row(old)?;
        if old.request_json != payload {
            return Err(DbError::Conflict("IDEMPOTENCY_KEY_REUSED".into()));
        }
        tx.commit().await?;
        return Ok(old);
    }
    let target = tx
        .query_one(stmt(TARGET_SQL, vec![request.conversation_id.into()]))
        .await?
        .ok_or_else(|| DbError::NotFound("CONVERSATION_NOT_FOUND".into()))?;
    let pending: Option<String> = target.try_get("", "switch_status")?;
    if matches!(pending.as_deref(), Some("queued" | "preparing")) {
        return Err(DbError::Conflict(
            "BUSINESS_SWITCH_PENDING: retry with the same key after handoff".into(),
        ));
    }
    let agent: String = target.try_get("", "agent_type")?;
    if let Some(required) = request.required_agent {
        if serde_json::to_value(required)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .as_deref()
            != Some(&agent)
        {
            return Err(DbError::Conflict("REQUIRED_AGENT_MISMATCH".into()));
        }
    }
    let external: Option<String> = target.try_get("", "external_id")?;
    let external = external.filter(|v| !v.is_empty()).ok_or_else(|| {
        DbError::Validation("SESSION_NOT_BOUND: initialize the conversation first".into())
    })?;
    let id = uuid::Uuid::new_v4().to_string();
    let deadline = (chrono::Utc::now()
        + chrono::Duration::seconds(i64::from(request.deadline_seconds)))
    .to_rfc3339();
    tx.execute(stmt("INSERT INTO business_delivery(id,conversation_id,client_message_id,request_json,target_agent,target_external_id,target_generation,submitted_revision,mode,deadline_at,target_settings) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
        vec![id.clone().into(),request.conversation_id.into(),request.client_message_id.clone().into(),payload.into(),agent.into(),external.into(),target.try_get::<String>("", "generation")?.into(),target.try_get::<i64>("", "revision")?.into(),request.mode.clone().into(),deadline.into(),serde_json::to_string(settings).map_err(|e| DbError::Validation(e.to_string()))?.into()])).await?;
    let result = row(tx
        .query_one(stmt(
            "SELECT * FROM business_delivery WHERE id=?",
            vec![id.into()],
        ))
        .await?
        .unwrap())?;
    tx.commit().await?;
    Ok(result)
}

pub async fn get(conn: &DatabaseConnection, cid: i32, id: &str) -> Result<Delivery, DbError> {
    conn.query_one(stmt(
        "SELECT * FROM business_delivery WHERE conversation_id=? AND (id=? OR client_message_id=?) ORDER BY (id=?) DESC LIMIT 1",
        vec![cid.into(), id.into(), id.into(), id.into()],
    ))
    .await?
    .map(row)
    .transpose()?
    .ok_or_else(|| DbError::NotFound("BUSINESS_MESSAGE_NOT_FOUND".into()))
}
pub async fn get_by_client_id(
    conn: &DatabaseConnection,
    cid: i32,
    id: &str,
) -> Result<Delivery, DbError> {
    conn.query_one(stmt(
        "SELECT * FROM business_delivery WHERE conversation_id=? AND client_message_id=?",
        vec![cid.into(), id.into()],
    ))
    .await?
    .map(row)
    .transpose()?
    .ok_or_else(|| DbError::NotFound("BUSINESS_MESSAGE_NOT_FOUND".into()))
}
pub async fn pending(
    conn: &DatabaseConnection,
    cid: Option<i32>,
) -> Result<Vec<Delivery>, DbError> {
    conn.query_all(stmt("SELECT * FROM business_delivery WHERE status IN ('queued','delivering','delivered','running','cancelling') AND (? IS NULL OR conversation_id=?) ORDER BY rowid", vec![cid.into(),cid.into()])).await?.into_iter().map(row).collect()
}
pub async fn transition(
    conn: &DatabaseConnection,
    delivery: &Delivery,
    status: &str,
    error: Option<String>,
    connection: Option<&str>,
    task: Option<&str>,
) -> Result<bool, DbError> {
    Ok(conn.execute(stmt("UPDATE business_delivery SET status=?,error=?,connection_id=COALESCE(?,connection_id),task_id=COALESCE(?,task_id),updated_at=CURRENT_TIMESTAMP WHERE id=? AND status=?",vec![status.into(),error.into(),connection.into(),task.into(),delivery.id.clone().into(),delivery.status.clone().into()])).await?.rows_affected()==1)
}
pub async fn validate_target<C: ConnectionTrait>(
    conn: &C,
    delivery: &Delivery,
) -> Result<(), DbError> {
    let target = conn
        .query_one(stmt(TARGET_SQL, vec![delivery.conversation_id.into()]))
        .await?
        .ok_or_else(|| DbError::NotFound("CONVERSATION_NOT_FOUND".into()))?;
    let agent: String = target.try_get("", "agent_type")?;
    let external: Option<String> = target.try_get("", "external_id")?;
    let generation: String = target.try_get("", "generation")?;
    let pending: Option<String> = target.try_get("", "switch_status")?;
    if matches!(pending.as_deref(), Some("queued" | "preparing"))
        || serde_json::to_value(delivery.target_agent)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .as_deref()
            != Some(&agent)
        || external.as_deref() != Some(delivery.target_external_id.as_str())
        || generation != delivery.target_generation
    {
        return Err(DbError::Conflict(
            "DELIVERY_TARGET_CHANGED: frozen target cannot be rerouted".into(),
        ));
    }
    Ok(())
}

pub async fn validate_prompt_target(
    conn: &DatabaseConnection,
    cid: i32,
    message_id: &str,
) -> Result<(), DbError> {
    if let Some(result) = conn
        .query_one(stmt(
            "SELECT * FROM business_delivery WHERE conversation_id=? AND client_message_id=?",
            vec![cid.into(), message_id.into()],
        ))
        .await?
    {
        let delivery = row(result)?;
        if delivery.status != "delivering" {
            return Err(DbError::Conflict("DELIVERY_ALREADY_CLAIMED".into()));
        }
        validate_target(conn, &delivery).await?;
    }
    Ok(())
}

pub async fn reconcile(conn: &DatabaseConnection, delivery: &Delivery) -> Result<bool, DbError> {
    if delivery.mode == "steer" && delivery.status == "delivering" {
        // The parent's existence cannot prove that this separate injection was accepted.
        return Ok(false);
    }
    let run = conn.query_one(stmt("SELECT id,status,prompt_accepted_at FROM conversation_turn_run WHERE conversation_id=? AND ((?='steer' AND id=?) OR (?<>'steer' AND client_message_id=?)) ORDER BY started_at DESC LIMIT 1",vec![delivery.conversation_id.into(),delivery.mode.clone().into(),delivery.task_id.clone().into(),delivery.mode.clone().into(),delivery.client_message_id.clone().into()])).await?;
    if let Some(run) = run {
        let native: String = run.try_get("", "status")?;
        let accepted: Option<String> = run.try_get("", "prompt_accepted_at")?;
        let status = match native.as_str() {
            "running" if accepted.is_some() => "running",
            "running" => "delivered",
            "cancelling" => "cancelling",
            "completed" => "completed",
            "cancelled" => "cancelled",
            _ => "failed",
        };
        let task: String = run.try_get("", "id")?;
        if delivery.status != status || delivery.task_id.as_deref() != Some(&task) {
            transition(
                conn,
                delivery,
                status,
                (status == "failed").then(|| format!("EXECUTION_{native}")),
                None,
                Some(&task),
            )
            .await?;
        }
        return Ok(true);
    }
    Ok(false)
}

pub async fn active_task_for_connection(
    conn: &DatabaseConnection,
    cid: i32,
    connection: &str,
) -> Result<Option<String>, DbError> {
    conn.query_one(stmt("SELECT id FROM conversation_turn_run WHERE conversation_id=? AND connection_id=? AND status IN ('running','cancelling') ORDER BY started_at DESC LIMIT 1",vec![cid.into(),connection.into()])).await?.map(|r|r.try_get("","id").map_err(DbError::from)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        service::{business_conversation_service as business, conversation_service},
        test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder},
    };
    async fn setup() -> (crate::db::AppDatabase, i32) {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/isolated-delivery").await;
        let cid = seed_conversation(&db, folder, AgentType::Pi).await;
        conversation_service::bind_external_id(&db.conn, cid, "pi-original", &[])
            .await
            .unwrap();
        let summary = conversation_service::get_by_id(&db.conn, cid)
            .await
            .unwrap();
        business::initialize(&db.conn, &summary, &[]).await.unwrap();
        (db, cid)
    }
    fn request(cid: i32, id: &str) -> SubmitRequest {
        SubmitRequest {
            conversation_id: cid,
            client_message_id: id.into(),
            blocks: vec![PromptInputBlock::Text {
                text: "do the business task".into(),
            }],
            required_agent: None,
            mode: "queue".into(),
            deadline_seconds: 3600,
            metadata: Default::default(),
        }
    }
    #[tokio::test]
    async fn business_delivery_is_idempotent_and_freezes_native_target() {
        let (db, cid) = setup().await;
        let req = request(cid, "stable-client-id");
        let one = submit(&db.conn, &req).await.unwrap();
        let two = submit(&db.conn, &req).await.unwrap();
        assert_eq!(one.id, two.id);
        assert_eq!(one.target_agent, AgentType::Pi);
        assert_eq!(one.target_external_id, "pi-original");
        assert_eq!(pending(&db.conn, Some(cid)).await.unwrap().len(), 1);
        let mut changed = req.clone();
        changed.blocks = vec![PromptInputBlock::Text {
            text: "different".into(),
        }];
        assert!(submit(&db.conn, &changed)
            .await
            .unwrap_err()
            .to_string()
            .contains("IDEMPOTENCY_KEY_REUSED"));
    }
    #[tokio::test]
    async fn business_delivery_client_ids_cannot_alias_another_receipt() {
        let (db, cid) = setup().await;
        let original = submit(&db.conn, &request(cid, "client-one")).await.unwrap();
        assert!(matches!(
            get_by_client_id(&db.conn, cid, &original.id).await,
            Err(DbError::NotFound(_))
        ));
        let next = submit(&db.conn, &request(cid, &original.id)).await.unwrap();
        assert_ne!(next.id, original.id);
        assert_eq!(
            get_by_client_id(&db.conn, cid, &original.id)
                .await
                .unwrap()
                .id,
            next.id
        );
        assert_eq!(
            get(&db.conn, cid, &original.id).await.unwrap().id,
            original.id
        );
        assert_eq!(pending(&db.conn, Some(cid)).await.unwrap().len(), 2);
    }
    #[tokio::test]
    async fn business_delivery_switching_is_an_explicit_rejection_and_does_not_reserve_key() {
        let (db, cid) = setup().await;
        let switch = business::AgentSwitchRequest {
            conversation_id: cid,
            request_id: "pi-to-codex".into(),
            target_agent: AgentType::Codex,
            expected_revision: Some(0),
            preferred_mode_id: None,
            preferred_config_values: Default::default(),
        };
        business::queue_switch(&db.conn, &switch).await.unwrap();
        let req = request(cid, "during-switch");
        assert!(submit(&db.conn, &req)
            .await
            .unwrap_err()
            .to_string()
            .contains("BUSINESS_SWITCH_PENDING"));
        assert!(pending(&db.conn, Some(cid)).await.unwrap().is_empty());
        business::set_switch_status(
            &db.conn,
            &switch,
            "failed",
            Some("adapter unavailable".into()),
        )
        .await
        .unwrap();
        assert_eq!(
            submit(&db.conn, &req).await.unwrap().target_agent,
            AgentType::Pi
        );
    }
    #[tokio::test]
    async fn business_delivery_required_agent_and_mode_are_checked_before_acceptance() {
        let (db, cid) = setup().await;
        let mut req = request(cid, "required");
        req.required_agent = Some(AgentType::Codex);
        assert!(submit(&db.conn, &req)
            .await
            .unwrap_err()
            .to_string()
            .contains("REQUIRED_AGENT_MISMATCH"));
        req.required_agent = Some(AgentType::Pi);
        req.mode = "follow_up".into();
        assert!(submit(&db.conn, &req)
            .await
            .unwrap_err()
            .to_string()
            .contains("DELIVERY_MODE_UNSUPPORTED"));
        req.mode = "steer".into();
        assert_eq!(submit(&db.conn, &req).await.unwrap().mode, "steer");
    }
    #[tokio::test]
    async fn business_delivery_old_target_is_never_reassigned_after_switch() {
        let (db, cid) = setup().await;
        let req = request(cid, "queued-pi-task");
        let one = submit(&db.conn, &req).await.unwrap();
        let switch = business::AgentSwitchRequest {
            conversation_id: cid,
            request_id: "switch-one".into(),
            target_agent: AgentType::Codex,
            expected_revision: Some(0),
            preferred_mode_id: None,
            preferred_config_values: Default::default(),
        };
        business::queue_switch(&db.conn, &switch).await.unwrap();
        business::activate(&db.conn, &switch, "codex-independent")
            .await
            .unwrap();
        assert!(validate_target(&db.conn, &one)
            .await
            .unwrap_err()
            .to_string()
            .contains("DELIVERY_TARGET_CHANGED"));
        let retry = submit(&db.conn, &req).await.unwrap();
        assert_eq!(retry.target_agent, AgentType::Pi);
        assert_eq!(retry.target_generation, one.target_generation);
        assert_eq!(retry.id, one.id);
    }
    #[tokio::test]
    async fn business_delivery_old_worker_cannot_overwrite_cancelled_or_new_status() {
        let (db, cid) = setup().await;
        let one = submit(&db.conn, &request(cid, "cancel-this"))
            .await
            .unwrap();
        assert!(transition(&db.conn, &one, "cancelled", None, None, None)
            .await
            .unwrap());
        assert!(!transition(
            &db.conn,
            &one,
            "delivering",
            None,
            Some("old-connection"),
            None
        )
        .await
        .unwrap());
        assert!(get(&db.conn, cid, &one.id).await.unwrap().terminal());
        assert!(pending(&db.conn, Some(cid)).await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn business_delivery_roundtrip_and_body_limits_survive_database_reload() {
        let (db, cid) = setup().await;
        let mut req = request(cid, "attachment");
        req.blocks.push(PromptInputBlock::Image {
            data: "".into(),
            mime_type: "image/png".into(),
            uri: Some("file:///uploads/input.png".into()),
        });
        req.metadata
            .insert("source".into(), "2k-compatibility-test-only".into());
        let one = submit(&db.conn, &req).await.unwrap();
        let loaded = get(&db.conn, cid, &req.client_message_id).await.unwrap();
        assert_eq!(one.request_json, loaded.request_json);
        assert_eq!(loaded.request().unwrap().blocks.len(), 2);
        let mut invalid = request(cid, "bad-deadline");
        invalid.deadline_seconds = 0;
        assert!(submit(&db.conn, &invalid).await.is_err());
    }

    #[tokio::test]
    async fn business_delivery_recovery_matches_the_owning_task_and_native_acceptance() {
        use crate::db::service::artifact_service::{create_run, mark_prompt_accepted, NewTurnRun};
        let (db, cid) = setup().await;
        let first = submit(&db.conn, &request(cid, "first-task")).await.unwrap();
        let next = submit(&db.conn, &request(cid, "next-task")).await.unwrap();
        for item in [&first, &next] {
            transition(
                &db.conn,
                item,
                "delivering",
                None,
                Some("live-connection"),
                None,
            )
            .await
            .unwrap();
        }
        create_run(
            &db.conn,
            NewTurnRun {
                id: "whole-native-task".into(),
                conversation_id: cid,
                connection_id: "live-connection".into(),
                client_message_id: Some(first.client_message_id.clone()),
                prompt_fingerprint: None,
                folder_id: None,
                root_path: "/isolated-delivery".into(),
                capture_incomplete: false,
                input_paths_json: "[]".into(),
                expectation_json: "{}".into(),
            },
        )
        .await
        .unwrap();
        let current = get(&db.conn, cid, &first.id).await.unwrap();
        assert!(reconcile(&db.conn, &current).await.unwrap());
        let current = get(&db.conn, cid, &first.id).await.unwrap();
        assert_eq!(current.status, "delivered");
        assert_eq!(current.task_id.as_deref(), Some("whole-native-task"));
        // A later queued message cannot inherit the previous task's state.
        assert!(
            !reconcile(&db.conn, &get(&db.conn, cid, &next.id).await.unwrap())
                .await
                .unwrap()
        );
        mark_prompt_accepted(&db.conn, "whole-native-task")
            .await
            .unwrap();
        reconcile(&db.conn, &current).await.unwrap();
        assert_eq!(
            get(&db.conn, cid, &first.id).await.unwrap().status,
            "running"
        );
        for (native, expected) in [
            ("completed", "completed"),
            ("failed", "failed"),
            ("cancelled", "cancelled"),
            ("interrupted", "failed"),
        ] {
            db.conn
                .execute(stmt(
                    "UPDATE conversation_turn_run SET status=? WHERE id=?",
                    vec![native.into(), "whole-native-task".into()],
                ))
                .await
                .unwrap();
            // Model separate restart snapshots for each native terminal outcome.
            db.conn
                .execute(stmt(
                    "UPDATE business_delivery SET status='delivered' WHERE id=?",
                    vec![first.id.clone().into()],
                ))
                .await
                .unwrap();
            reconcile(&db.conn, &get(&db.conn, cid, &first.id).await.unwrap())
                .await
                .unwrap();
            assert_eq!(
                get(&db.conn, cid, &first.id).await.unwrap().status,
                expected
            );
            assert_eq!(
                get(&db.conn, cid, &next.id).await.unwrap().status,
                "delivering"
            );
        }
    }

    #[test]
    fn business_delivery_configuration_keeps_tier_and_reasoning_independent() {
        use crate::acp::{
            session_state::SessionState,
            types::{SessionConfigKindInfo, SessionConfigOptionInfo, SessionConfigSelectInfo},
        };
        let mut state = SessionState::new(
            "pi-queue".into(),
            AgentType::Pi,
            None,
            "isolated-test".into(),
            None,
        );
        let settings = ExecutionSettings {
            mode: None,
            config: BTreeMap::from([
                ("model".into(), "fixture/gpt-5.4".into()),
                ("service_tier".into(), "on".into()),
                ("thinking".into(), "medium".into()),
            ]),
        };
        state.config_options = Some(
            settings
                .config
                .iter()
                .map(|(id, value)| SessionConfigOptionInfo {
                    id: id.clone(),
                    name: id.clone(),
                    description: None,
                    category: None,
                    recommended_value: None,
                    kind: SessionConfigKindInfo::Select(SessionConfigSelectInfo {
                        current_value: value.clone(),
                        options: vec![],
                        groups: vec![],
                    }),
                })
                .collect(),
        );
        assert!(settings.matches(&state));
        let snapshot = ExecutionSettings::snapshot(&state);
        assert_eq!(snapshot.config["thinking"], "medium");
        let mut changed = settings.clone();
        changed.config.insert("service_tier".into(), "off".into());
        assert!(!changed.matches(&state));
        changed = settings.clone();
        changed.config.insert("thinking".into(), "high".into());
        assert!(!changed.matches(&state));
        let restored: ExecutionSettings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored, settings);
    }
}
