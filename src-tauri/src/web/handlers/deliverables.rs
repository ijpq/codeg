use std::path::PathBuf;
use std::sync::Arc;

use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use axum::body::Body;
use axum::extract::{Extension, Path as AxumPath};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_lite::io::AsyncWriteExt as _;
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::deliverables::{self, DeliverableDownloadRequest, DeliverableIdsRequest};
use crate::db::service::deliverable_service::{self, ResolvedDeliverable};
use crate::workspace_transfer::DeliverableDownloadKind;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRequest {
    pub conversation_id: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnRequest {
    pub conversation_id: i32,
    pub turn_run_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SingleDeliverableRequest {
    pub conversation_id: i32,
    pub deliverable_id: String,
}

pub async fn capabilities() -> Json<deliverables::DeliverableCapabilities> {
    Json(deliverables::deliverable_capabilities_core())
}

pub async fn list_for_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(request): Json<ConversationRequest>,
) -> Result<Json<Vec<crate::models::ConversationDeliverable>>, AppCommandError> {
    Ok(Json(
        deliverables::list_conversation_deliverables_core(&state.db.conn, request.conversation_id)
            .await?,
    ))
}

pub async fn list_for_turn(
    Extension(state): Extension<Arc<AppState>>,
    Json(request): Json<TurnRequest>,
) -> Result<Json<Vec<crate::models::ConversationDeliverable>>, AppCommandError> {
    Ok(Json(
        deliverables::list_turn_deliverables_core(
            &state.db.conn,
            request.conversation_id,
            &request.turn_run_id,
        )
        .await?,
    ))
}

pub async fn list_runs_for_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(request): Json<ConversationRequest>,
) -> Result<Json<Vec<crate::models::ConversationTurnDeliverableSet>>, AppCommandError> {
    Ok(Json(
        deliverables::list_conversation_deliverable_runs_core(
            &state.db.conn,
            request.conversation_id,
        )
        .await?,
    ))
}

pub async fn create_download_ticket(
    Extension(state): Extension<Arc<AppState>>,
    Json(request): Json<DeliverableDownloadRequest>,
) -> Result<Json<crate::workspace_transfer::DownloadTicketIssued>, AppCommandError> {
    Ok(Json(
        deliverables::create_deliverable_download_ticket_core(
            &state.db.conn,
            state.workspace_transfer.clone(),
            request,
            "/api/deliverable_download",
        )
        .await?,
    ))
}

pub async fn copy(
    Extension(state): Extension<Arc<AppState>>,
    Json(request): Json<DeliverableIdsRequest>,
) -> Result<Json<deliverables::DeliverableOperationResult>, AppCommandError> {
    Ok(Json(
        deliverables::copy_deliverables_core(&state.db.conn, request).await?,
    ))
}

pub async fn reveal(
    Extension(state): Extension<Arc<AppState>>,
    Json(request): Json<SingleDeliverableRequest>,
) -> Result<Json<deliverables::DeliverableOperationResult>, AppCommandError> {
    Ok(Json(
        deliverables::reveal_deliverable_core(
            &state.db.conn,
            request.conversation_id,
            request.deliverable_id,
        )
        .await?,
    ))
}

pub async fn open(
    Extension(state): Extension<Arc<AppState>>,
    Json(request): Json<SingleDeliverableRequest>,
) -> Result<Json<deliverables::DeliverableOperationResult>, AppCommandError> {
    Ok(Json(
        deliverables::open_deliverable_core(
            &state.db.conn,
            request.conversation_id,
            request.deliverable_id,
        )
        .await?,
    ))
}

pub async fn hide(
    Extension(state): Extension<Arc<AppState>>,
    Json(request): Json<DeliverableIdsRequest>,
) -> Result<Json<deliverables::DeliverableOperationResult>, AppCommandError> {
    let conversation_id = request.conversation_id;
    let result = deliverables::hide_deliverables_core(&state.db.conn, request).await?;
    crate::acp::deliverables::emit_deliverables_changed(
        &state.emitter,
        conversation_id,
        Vec::new(),
    );
    Ok(Json(result))
}

pub async fn consume_download_ticket(
    Extension(state): Extension<Arc<AppState>>,
    AxumPath(ticket): AxumPath<String>,
) -> Result<Response, AppCommandError> {
    let Some(ticket) = state
        .workspace_transfer
        .consume_deliverable_download_ticket(&ticket)
        .await
    else {
        return Err(AppCommandError::not_found(
            "Deliverable download ticket is invalid or expired",
        ));
    };
    let resolved = deliverable_service::resolve_for_access(
        &state.db.conn,
        ticket.conversation_id,
        &ticket.deliverable_ids,
    )
    .await
    .map_err(|error| match error {
        crate::db::error::DbError::NotFound(message) => AppCommandError::not_found(message),
        crate::db::error::DbError::Validation(message) => AppCommandError::invalid_input(message),
        crate::db::error::DbError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            AppCommandError::not_found("Deliverable file no longer exists")
        }
        crate::db::error::DbError::Io(error) => AppCommandError::io(error),
        other => AppCommandError::db(other),
    })?;
    match ticket.kind {
        DeliverableDownloadKind::Single => {
            if resolved.len() != 1 || resolved[0].model.kind != "file" {
                return Err(AppCommandError::invalid_input(
                    "The deliverable is not a downloadable file",
                ));
            }
            super::workspace_files::stream_file_response(
                &resolved[0].absolute_path,
                &ticket.filename,
            )
            .await
        }
        DeliverableDownloadKind::Zip => Ok(stream_deliverables_zip(
            state.workspace_transfer.clone(),
            resolved,
            ticket.filename,
        )),
    }
}

fn stream_deliverables_zip(
    manager: Arc<crate::workspace_transfer::WorkspaceTransferManager>,
    items: Vec<ResolvedDeliverable>,
    filename: String,
) -> Response {
    let (reader, writer) = tokio::io::duplex(8 * 64 * 1024);
    tokio::spawn(async move {
        if let Err(error) = write_deliverables_zip_stream(manager, items, writer).await {
            tracing::error!("[deliverables] streaming ZIP failed: {error}");
        }
    });
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    if let Some(value) = super::workspace_files::attachment_header(&filename) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    (
        StatusCode::OK,
        headers,
        Body::from_stream(ReaderStream::with_capacity(reader, 64 * 1024)),
    )
        .into_response()
}

async fn add_file_to_zip(
    writer: &mut ZipFileWriter<tokio::io::DuplexStream>,
    source: PathBuf,
    entry_name: String,
) -> Result<(), AppCommandError> {
    let entry =
        ZipEntryBuilder::new(entry_name.into(), Compression::Deflate).unix_permissions(0o644);
    let mut entry_writer = writer.write_entry_stream(entry).await.map_err(|error| {
        AppCommandError::io_error("Failed to start deliverable ZIP entry")
            .with_detail(error.to_string())
    })?;
    let mut file = tokio::fs::File::open(source)
        .await
        .map_err(AppCommandError::io)?;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await.map_err(AppCommandError::io)?;
        if read == 0 {
            break;
        }
        entry_writer
            .write_all(&buffer[..read])
            .await
            .map_err(|error| {
                AppCommandError::io_error("Failed to stream deliverable ZIP entry")
                    .with_detail(error.to_string())
            })?;
    }
    entry_writer.close().await.map_err(|error| {
        AppCommandError::io_error("Failed to close deliverable ZIP entry")
            .with_detail(error.to_string())
    })
}

async fn write_deliverables_zip_stream(
    manager: Arc<crate::workspace_transfer::WorkspaceTransferManager>,
    items: Vec<ResolvedDeliverable>,
    sink: tokio::io::DuplexStream,
) -> Result<(), AppCommandError> {
    let _permit = manager.zip_semaphore.acquire().await.map_err(|_| {
        AppCommandError::task_execution_failed("Deliverable ZIP semaphore is closed")
    })?;
    let names = deliverables::predictable_archive_names(&items);
    let mut writer = ZipFileWriter::with_tokio(sink);
    for (item, name) in items.into_iter().zip(names) {
        if item.model.kind == "file" {
            add_file_to_zip(&mut writer, item.absolute_path, name).await?;
            continue;
        }
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(&item.absolute_path).follow_links(false) {
            let entry = entry.map_err(|error| {
                AppCommandError::io_error("Failed to walk deliverable directory")
                    .with_detail(error.to_string())
            })?;
            if entry.file_type().is_file() && !entry.file_type().is_symlink() {
                let relative = entry
                    .path()
                    .strip_prefix(&item.absolute_path)
                    .map_err(|error| {
                        AppCommandError::invalid_input(format!(
                            "Invalid deliverable directory entry: {error}"
                        ))
                    })?
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push((entry.path().to_path_buf(), format!("{name}/{relative}")));
            }
        }
        for (path, entry_name) in files {
            add_file_to_zip(&mut writer, path, entry_name).await?;
        }
    }
    writer.close().await.map(|_| ()).map_err(|error| {
        AppCommandError::io_error("Failed to finalize deliverables ZIP")
            .with_detail(error.to_string())
    })
}
