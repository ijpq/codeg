//! Backend-owned delivery supervisor; HTTP disconnects never cancel accepted work.
use crate::{
    acp::{error::AcpError, manager::ConnectionManager, types::ConnectionStatus},
    db::{
        service::{business_delivery_service as delivery, conversation_service},
        AppDatabase,
    },
    web::event_bridge::EventEmitter,
};
use std::{collections::HashSet, path::Path, sync::OnceLock, time::Duration};

type WorkerKey = (usize, i32);
fn workers() -> &'static tokio::sync::Mutex<HashSet<WorkerKey>> {
    static WORKERS: OnceLock<tokio::sync::Mutex<HashSet<WorkerKey>>> = OnceLock::new();
    WORKERS.get_or_init(Default::default)
}
fn error(e: impl std::fmt::Display) -> AcpError {
    AcpError::protocol(e.to_string())
}

pub async fn submit_core(
    request: delivery::SubmitRequest,
    manager: &ConnectionManager,
    db: &AppDatabase,
    data_dir: &Path,
    emitter: EventEmitter,
) -> Result<delivery::Delivery, AcpError> {
    // A retry only reads its durable receipt. It must not restore/switch an engine.
    match delivery::get_by_client_id(
        &db.conn,
        request.conversation_id,
        &request.client_message_id,
    )
    .await
    {
        Ok(_) => return delivery::submit(&db.conn, &request).await.map_err(error),
        Err(crate::db::error::DbError::NotFound(_)) => {}
        Err(e) => return Err(error(e)),
    }
    let restored = crate::commands::acp::acp_restore_conversation_core(
        request.conversation_id,
        None,
        Default::default(),
        manager,
        db,
        data_dir,
        "business-delivery-accept".into(),
        emitter.clone(),
    )
    .await?;
    let _guard = manager
        .lock_restore_request(request.conversation_id)
        .await?;
    let state = manager
        .get_state(&restored.connection_id)
        .await
        .ok_or_else(|| error("DELIVERY_CONNECTION_LOST"))?;
    let state = state.read().await;
    if request.mode == "steer" && !state.supports_steer {
        return Err(error(
            "DELIVERY_MODE_UNSUPPORTED: native steer is unavailable",
        ));
    }
    let settings = delivery::ExecutionSettings::snapshot(&state);
    drop(state);
    let result = delivery::submit_with_settings(&db.conn, &request, &settings)
        .await
        .map_err(error)?;
    drop(_guard);
    if !result.terminal() {
        ensure_worker(result.conversation_id, manager, db, data_dir, emitter).await;
    }
    Ok(result)
}

pub async fn ensure_worker(
    cid: i32,
    manager: &ConnectionManager,
    db: &AppDatabase,
    data_dir: &Path,
    emitter: EventEmitter,
) {
    let key = (manager.instance_identity(), cid);
    if !workers().lock().await.insert(key) {
        return;
    }
    let manager = manager.clone_ref();
    let db = db.clone();
    let data_dir = data_dir.to_owned();
    tokio::spawn(async move {
        let mut failures = 0;
        loop {
            let result = advance(cid, &manager, &db, &data_dir, emitter.clone()).await;
            match result {
                Ok(true) => {
                    failures = 0;
                    tokio::time::sleep(Duration::from_millis(500)).await
                }
                Ok(false) => {
                    workers().lock().await.remove(&key);
                    if delivery::pending(&db.conn, Some(cid))
                        .await
                        .is_ok_and(|v| !v.is_empty())
                        && workers().lock().await.insert(key)
                    {
                        continue;
                    }
                    return;
                }
                Err(e) => {
                    failures += 1;
                    tracing::error!(conversation_id=cid,error=%e,failures,"[business-delivery] supervisor failed; receipts retained");
                    if failures < 3 {
                        tokio::time::sleep(Duration::from_secs(failures)).await;
                        continue;
                    }
                    if let Ok(items) = delivery::pending(&db.conn, Some(cid)).await {
                        for item in items {
                            let status = if item.status == "queued" {
                                "failed"
                            } else {
                                "uncertain"
                            };
                            let _ = delivery::transition(&db.conn, &item, status, Some("DELIVERY_SUPERVISOR_FAILED: inspect receipt before retrying with a new key".into()), None, None).await;
                        }
                    }
                    workers().lock().await.remove(&key);
                    return;
                }
            }
        }
    });
}

pub async fn recover(
    manager: &ConnectionManager,
    db: &AppDatabase,
    data_dir: &Path,
    emitter: EventEmitter,
) {
    match delivery::pending(&db.conn, None).await {
        Ok(items) => {
            let mut ids = HashSet::new();
            for item in items {
                if ids.insert(item.conversation_id) {
                    ensure_worker(item.conversation_id, manager, db, data_dir, emitter.clone())
                        .await;
                }
            }
        }
        Err(e) => tracing::error!(error=%e,"[business-delivery] recovery failed"),
    }
}

async fn advance(
    cid: i32,
    manager: &ConnectionManager,
    db: &AppDatabase,
    data_dir: &Path,
    emitter: EventEmitter,
) -> Result<bool, AcpError> {
    let mut pending = delivery::pending(&db.conn, Some(cid))
        .await
        .map_err(error)?;
    // Reconcile running receipts independently: they must never block a steer
    // that belongs to that same active task, or the next ordinary queue item.
    for item in pending.iter().filter(|item| item.status != "queued") {
        if !delivery::reconcile(&db.conn, item).await.map_err(error)? {
            delivery::transition(
                &db.conn,
                item,
                "uncertain",
                Some(
                    "DELIVERY_OUTCOME_UNKNOWN: inspect native history before submitting a new key"
                        .into(),
                ),
                None,
                None,
            )
            .await
            .map_err(error)?;
        } else {
            let current = delivery::get(&db.conn, cid, &item.id)
                .await
                .map_err(error)?;
            // A later queue item must never starve the owning task's deadline.
            // Enforce it before choosing which pending message to deliver next.
            if !current.terminal() && chrono::Utc::now().to_rfc3339() > current.deadline_at {
                if current.mode == "steer" {
                    delivery::transition(
                        &db.conn,
                        &current,
                        "failed",
                        Some("DELIVERY_DEADLINE_EXCEEDED: owning task was not cancelled".into()),
                        None,
                        None,
                    )
                    .await
                    .map_err(error)?;
                } else {
                    cancel_core(cid, &current.id, manager, db).await?;
                }
            }
        }
    }
    pending = delivery::pending(&db.conn, Some(cid))
        .await
        .map_err(error)?;
    let Some(item) = pending
        .iter()
        .find(|item| item.status == "queued" && item.mode == "steer")
        .or_else(|| pending.iter().find(|item| item.status == "queued"))
        .or_else(|| pending.first())
        .cloned()
    else {
        return Ok(false);
    };
    if item.status != "queued" {
        if delivery::reconcile(&db.conn, &item).await.map_err(error)? {
            let current = delivery::get(&db.conn, cid, &item.id)
                .await
                .map_err(error)?;
            if !current.terminal() && chrono::Utc::now().to_rfc3339() > item.deadline_at {
                if item.mode == "steer" {
                    delivery::transition(
                        &db.conn,
                        &current,
                        "failed",
                        Some("DELIVERY_DEADLINE_EXCEEDED: owning task was not cancelled".into()),
                        None,
                        None,
                    )
                    .await
                    .map_err(error)?;
                } else {
                    cancel_core(cid, &item.id, manager, db).await?;
                }
            }
            return Ok(true);
        }
        // A crash can occur after the CLI accepted work but before a receipt was
        // written. Never replay this window as if nothing happened.
        delivery::transition(
            &db.conn,
            &item,
            "uncertain",
            Some(
                "DELIVERY_OUTCOME_UNKNOWN: inspect native history before submitting a new key"
                    .into(),
            ),
            None,
            None,
        )
        .await
        .map_err(error)?;
        return Ok(true);
    }
    if chrono::Utc::now().to_rfc3339() > item.deadline_at {
        delivery::transition(
            &db.conn,
            &item,
            "failed",
            Some("DELIVERY_DEADLINE_EXCEEDED".into()),
            None,
            None,
        )
        .await
        .map_err(error)?;
        return Ok(true);
    }
    if let Err(e) = delivery::validate_target(&db.conn, &item).await {
        delivery::transition(&db.conn, &item, "failed", Some(e.to_string()), None, None)
            .await
            .map_err(error)?;
        return Ok(true);
    }
    let restored = crate::commands::acp::acp_restore_conversation_core(
        cid,
        None,
        Default::default(),
        manager,
        db,
        data_dir,
        "business-delivery".into(),
        emitter,
    )
    .await;
    let restored = match restored {
        Ok(restored) => restored,
        // At a native turn boundary the writer may still be retiring while
        // durable run reconciliation has already advanced. Keep an ordinary
        // queue item pending; its original deadline still bounds this retry.
        Err(AcpError::TurnInProgress) if item.mode == "queue" => return Ok(true),
        Err(e) => {
            delivery::transition(
                &db.conn,
                &item,
                "failed",
                Some(format!("DELIVERY_PREPARE_FAILED: {e}")),
                None,
                None,
            )
            .await
            .map_err(error)?;
            return Ok(true);
        }
    };
    let Some(state) = manager.get_state(&restored.connection_id).await else {
        return Err(error("DELIVERY_CONNECTION_LOST"));
    };
    let (busy, supports_steer) = {
        let state = state.read().await;
        if !item.target_settings.matches(&state) {
            delivery::transition(&db.conn, &item, "failed", Some("DELIVERY_SETTINGS_CHANGED: model, tier or session settings differ from acceptance".into()), None, None).await.map_err(error)?;
            return Ok(true);
        }
        (
            state.turn_in_flight || state.status == ConnectionStatus::Prompting,
            state.supports_steer,
        )
    };
    if item.mode == "queue" && busy {
        return Ok(true);
    }
    if item.mode == "steer" && (!busy || !supports_steer) {
        delivery::transition(
            &db.conn,
            &item,
            "failed",
            Some(
                if busy {
                    "STEER_UNSUPPORTED"
                } else {
                    "STEER_NO_ACTIVE_TURN"
                }
                .into(),
            ),
            None,
            None,
        )
        .await
        .map_err(error)?;
        return Ok(true);
    }
    if !delivery::transition(
        &db.conn,
        &item,
        "delivering",
        None,
        Some(&restored.connection_id),
        None,
    )
    .await
    .map_err(error)?
    {
        return Ok(true);
    }
    let request = item.request().map_err(error)?;
    let result = if item.mode == "steer" {
        let _guard = manager.lock_restore_request(cid).await?;
        if let Err(e) = delivery::validate_target(&db.conn, &item).await {
            let current = delivery::get(&db.conn, cid, &item.id)
                .await
                .map_err(error)?;
            delivery::transition(
                &db.conn,
                &current,
                "failed",
                Some(e.to_string()),
                None,
                None,
            )
            .await
            .map_err(error)?;
            return Ok(true);
        }
        let run = delivery::active_task_for_connection(&db.conn, cid, &restored.connection_id)
            .await
            .map_err(error)?;
        if let Some(run) = run {
            let current = delivery::get(&db.conn, cid, &item.id)
                .await
                .map_err(error)?;
            delivery::transition(&db.conn, &current, "delivering", None, None, Some(&run))
                .await
                .map_err(error)?;
        }
        manager
            .steer(
                &restored.connection_id,
                request.blocks,
                request.client_message_id,
            )
            .await
            .map(|_| ())
    } else {
        let summary = conversation_service::get_by_id(&db.conn, cid)
            .await
            .map_err(error)?;
        manager
            .send_prompt_linked_with_message_id(
                db,
                &restored.connection_id,
                request.blocks,
                Some(summary.folder_id),
                Some(cid),
                None,
                Some(request.client_message_id),
            )
            .await
            .map(|_| ())
    };
    let current = delivery::get(&db.conn, cid, &item.id)
        .await
        .map_err(error)?;
    match result {
        Ok(()) => {
            delivery::transition(&db.conn, &current, "delivered", None, None, None)
                .await
                .map_err(error)?;
        }
        Err(AcpError::TurnInProgress) if item.mode == "queue" => {
            delivery::transition(&db.conn, &current, "queued", None, None, None)
                .await
                .map_err(error)?;
        }
        Err(e) => {
            delivery::transition(
                &db.conn,
                &current,
                "failed",
                Some(format!("DELIVERY_FAILED: {e}")),
                None,
                None,
            )
            .await
            .map_err(error)?;
        }
    }
    Ok(true)
}

pub async fn cancel_core(
    cid: i32,
    id: &str,
    manager: &ConnectionManager,
    db: &AppDatabase,
) -> Result<delivery::Delivery, AcpError> {
    let _guard = manager.lock_restore_request(cid).await?;
    let mut item = delivery::get(&db.conn, cid, id).await.map_err(error)?;
    if matches!(
        item.status.as_str(),
        "delivering" | "delivered" | "running" | "cancelling"
    ) {
        delivery::reconcile(&db.conn, &item).await.map_err(error)?;
        item = delivery::get(&db.conn, cid, id).await.map_err(error)?;
    }
    if item.terminal() {
        return Ok(item);
    }
    if item.mode == "steer" && item.status != "queued" {
        return Err(error("DELIVERY_MODE_UNSUPPORTED: accepted steering cannot be individually retracted; cancel the owning task explicitly"));
    }
    if item.status == "queued" {
        delivery::transition(&db.conn, &item, "cancelled", None, None, None)
            .await
            .map_err(error)?;
    } else if let Some(connection) = item.connection_id.as_deref() {
        let active = delivery::active_task_for_connection(&db.conn, cid, connection)
            .await
            .map_err(error)?;
        if active.is_some() && active == item.task_id {
            manager
                .cancel_with_origin(&db.conn, connection, "business_delivery_api", None)
                .await?;
            delivery::transition(&db.conn, &item, "cancelling", None, None, None)
                .await
                .map_err(error)?;
        } else if !delivery::reconcile(&db.conn, &item).await.map_err(error)? {
            delivery::transition(
                &db.conn,
                &item,
                "uncertain",
                Some("CANCEL_OUTCOME_UNKNOWN".into()),
                None,
                None,
            )
            .await
            .map_err(error)?;
        }
    }
    delivery::get(&db.conn, cid, id).await.map_err(error)
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn submit_business_message(
    request: delivery::SubmitRequest,
    manager: tauri::State<'_, ConnectionManager>,
    db: tauri::State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<delivery::Delivery, AcpError> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_data_dir()
        .map(|p| crate::paths::resolve_effective_data_dir(&p))
        .map_err(error)?;
    submit_core(request, &manager, &db, &dir, EventEmitter::Tauri(app)).await
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn get_business_message(
    conversation_id: i32,
    message_id: String,
    db: tauri::State<'_, AppDatabase>,
) -> Result<delivery::Delivery, AcpError> {
    let item = delivery::get(&db.conn, conversation_id, &message_id)
        .await
        .map_err(error)?;
    if matches!(item.status.as_str(), "delivered" | "running" | "cancelling") {
        delivery::reconcile(&db.conn, &item).await.map_err(error)?;
    }
    delivery::get(&db.conn, conversation_id, &message_id)
        .await
        .map_err(error)
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn cancel_business_message(
    conversation_id: i32,
    message_id: String,
    manager: tauri::State<'_, ConnectionManager>,
    db: tauri::State<'_, AppDatabase>,
) -> Result<delivery::Delivery, AcpError> {
    cancel_core(conversation_id, &message_id, &manager, &db).await
}
