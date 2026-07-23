use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::app_error::AppCommandError;
use crate::db::error::DbError;
use crate::db::service::deliverable_service::{self, ResolvedDeliverable};
use crate::models::{ConversationDeliverable, ConversationTurnDeliverableSet};
use crate::workspace_transfer::{
    DeliverableDownloadKind, DeliverableDownloadTicketSpec, DownloadTicketIssued,
    WorkspaceTransferManager,
};

const MAX_BATCH_DELIVERABLES: usize = 100;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliverableIdsRequest {
    pub conversation_id: i32,
    pub deliverable_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliverableDownloadRequest {
    pub conversation_id: i32,
    pub deliverable_ids: Vec<String>,
    #[serde(default)]
    pub archive: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliverableCapabilities {
    pub host_os: &'static str,
    pub open_with_default_app: bool,
    pub copy_files: bool,
    pub reveal_in_folder: bool,
    pub host_action_notice: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliverableOperationResult {
    pub affected: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliverableSaveResult {
    pub saved_path: String,
    pub bytes: u64,
}

fn map_db_error(error: DbError) -> AppCommandError {
    match error {
        DbError::NotFound(message) => AppCommandError::not_found(message),
        DbError::Validation(message) => AppCommandError::invalid_input(message),
        DbError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            AppCommandError::not_found("Deliverable file no longer exists")
        }
        DbError::Io(error) => AppCommandError::io(error),
        other => AppCommandError::db(other),
    }
}

fn validate_ids(ids: &[String]) -> Result<(), AppCommandError> {
    if ids.is_empty() {
        return Err(AppCommandError::invalid_input(
            "Select at least one deliverable",
        ));
    }
    if ids.len() > MAX_BATCH_DELIVERABLES {
        return Err(AppCommandError::invalid_input(format!(
            "At most {MAX_BATCH_DELIVERABLES} deliverables may be selected"
        )));
    }
    if ids.iter().any(|id| id.trim().is_empty()) {
        return Err(AppCommandError::invalid_input(
            "Deliverable ids must not be empty",
        ));
    }
    Ok(())
}

pub fn deliverable_capabilities_core() -> DeliverableCapabilities {
    let open_with_default_app = can_open_with_default_app();
    DeliverableCapabilities {
        host_os: std::env::consts::OS,
        open_with_default_app,
        copy_files: cfg!(target_os = "windows"),
        reveal_in_folder: cfg!(target_os = "windows"),
        // Native operations intentionally target the Codeg host, not a
        // remote browser/device. Only show the notice when this host exposes
        // at least one such operation.
        host_action_notice: open_with_default_app || cfg!(target_os = "windows"),
    }
}

pub async fn list_conversation_deliverables_core(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<ConversationDeliverable>, AppCommandError> {
    deliverable_service::list_for_conversation(conn, conversation_id)
        .await
        .map_err(map_db_error)
}

pub async fn list_turn_deliverables_core(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turn_run_id: &str,
) -> Result<Vec<ConversationDeliverable>, AppCommandError> {
    deliverable_service::list_for_turn(conn, conversation_id, turn_run_id)
        .await
        .map_err(map_db_error)
}

pub async fn list_conversation_deliverable_runs_core(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<ConversationTurnDeliverableSet>, AppCommandError> {
    deliverable_service::list_sets_for_conversation(conn, conversation_id)
        .await
        .map_err(map_db_error)
}

pub async fn create_deliverable_download_ticket_core(
    conn: &DatabaseConnection,
    manager: Arc<WorkspaceTransferManager>,
    request: DeliverableDownloadRequest,
    base_url: &str,
) -> Result<DownloadTicketIssued, AppCommandError> {
    validate_ids(&request.deliverable_ids)?;
    let resolved = deliverable_service::resolve_for_access(
        conn,
        request.conversation_id,
        &request.deliverable_ids,
    )
    .await
    .map_err(map_db_error)?;
    let archive = request.archive || resolved.len() != 1 || resolved[0].model.kind == "directory";
    let filename = if archive {
        format!("codeg-deliverables-{}.zip", request.conversation_id)
    } else {
        resolved[0].model.file_name.clone()
    };
    let issued = manager
        .issue_deliverable_download_ticket(DeliverableDownloadTicketSpec {
            conversation_id: request.conversation_id,
            deliverable_ids: request.deliverable_ids,
            kind: if archive {
                DeliverableDownloadKind::Zip
            } else {
                DeliverableDownloadKind::Single
            },
            filename,
        })
        .await;
    Ok(DownloadTicketIssued {
        url: format!("{}/{}", base_url.trim_end_matches('/'), issued.ticket),
        ..issued
    })
}

pub async fn copy_deliverables_core(
    conn: &DatabaseConnection,
    request: DeliverableIdsRequest,
) -> Result<DeliverableOperationResult, AppCommandError> {
    validate_ids(&request.deliverable_ids)?;
    let resolved = deliverable_service::resolve_for_access(
        conn,
        request.conversation_id,
        &request.deliverable_ids,
    )
    .await
    .map_err(map_db_error)?;
    let paths = resolved
        .into_iter()
        .map(|item| item.absolute_path)
        .collect::<Vec<_>>();
    copy_paths_to_host_clipboard(paths).await?;
    Ok(DeliverableOperationResult {
        affected: request.deliverable_ids.len(),
    })
}

pub async fn reveal_deliverable_core(
    conn: &DatabaseConnection,
    conversation_id: i32,
    deliverable_id: String,
) -> Result<DeliverableOperationResult, AppCommandError> {
    validate_ids(std::slice::from_ref(&deliverable_id))?;
    let resolved = deliverable_service::resolve_for_access(
        conn,
        conversation_id,
        std::slice::from_ref(&deliverable_id),
    )
    .await
    .map_err(map_db_error)?;
    reveal_path_on_host(&resolved[0].absolute_path)?;
    Ok(DeliverableOperationResult { affected: 1 })
}

pub async fn open_deliverable_core(
    conn: &DatabaseConnection,
    conversation_id: i32,
    deliverable_id: String,
) -> Result<DeliverableOperationResult, AppCommandError> {
    validate_ids(std::slice::from_ref(&deliverable_id))?;
    let resolved = deliverable_service::resolve_for_access(
        conn,
        conversation_id,
        std::slice::from_ref(&deliverable_id),
    )
    .await
    .map_err(map_db_error)?;
    let path = resolved[0].absolute_path.clone();
    tokio::task::spawn_blocking(move || open_path_with_default_app(&path))
        .await
        .map_err(|error| {
            AppCommandError::task_execution_failed("Default application launcher stopped")
                .with_detail(error.to_string())
        })??;
    Ok(DeliverableOperationResult { affected: 1 })
}

pub async fn hide_deliverables_core(
    conn: &DatabaseConnection,
    request: DeliverableIdsRequest,
) -> Result<DeliverableOperationResult, AppCommandError> {
    validate_ids(&request.deliverable_ids)?;
    // Missing files are still removable, so the service verifies all ids and
    // applies the hide in one transaction without requiring file availability.
    let affected = deliverable_service::hide_for_conversation(
        conn,
        request.conversation_id,
        &request.deliverable_ids,
    )
    .await
    .map_err(map_db_error)?;
    if affected != request.deliverable_ids.len() as u64 {
        return Err(AppCommandError::not_found(
            "One or more deliverables do not belong to this conversation",
        ));
    }
    Ok(DeliverableOperationResult {
        affected: affected as usize,
    })
}

pub(crate) fn predictable_archive_names(items: &[ResolvedDeliverable]) -> Vec<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut used = HashSet::new();
    items
        .iter()
        .map(|item| {
            let original = item.model.file_name.clone();
            let path = Path::new(&original);
            let stem = path
                .file_stem()
                .map(|value| value.to_string_lossy().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "deliverable".into());
            let extension = path
                .extension()
                .map(|value| value.to_string_lossy().to_string());
            let key = if cfg!(windows) {
                original.to_lowercase()
            } else {
                original.clone()
            };
            let count = counts.entry(key).or_insert(0);
            loop {
                *count += 1;
                let candidate = if *count == 1 {
                    original.clone()
                } else if let Some(extension) = &extension {
                    format!("{stem} ({count}).{extension}")
                } else {
                    format!("{stem} ({count})")
                };
                let uniqueness = if cfg!(windows) {
                    candidate.to_lowercase()
                } else {
                    candidate.clone()
                };
                if used.insert(uniqueness) {
                    break candidate;
                }
            }
        })
        .collect()
}

pub fn write_deliverables_zip(
    items: &[ResolvedDeliverable],
    destination: &Path,
) -> Result<u64, AppCommandError> {
    let file = std::fs::File::create(destination).map_err(AppCommandError::io)?;
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    let names = predictable_archive_names(items);
    for (item, name) in items.iter().zip(names) {
        if item.model.kind == "file" {
            archive.start_file(name, options).map_err(|error| {
                AppCommandError::io_error("Failed to create ZIP entry")
                    .with_detail(error.to_string())
            })?;
            let mut source =
                std::fs::File::open(&item.absolute_path).map_err(AppCommandError::io)?;
            std::io::copy(&mut source, &mut archive).map_err(AppCommandError::io)?;
            continue;
        }
        for entry in walkdir::WalkDir::new(&item.absolute_path).follow_links(false) {
            let entry = entry.map_err(|error| {
                AppCommandError::io_error("Failed to walk deliverable directory")
                    .with_detail(error.to_string())
            })?;
            if entry.file_type().is_symlink() || !entry.file_type().is_file() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&item.absolute_path)
                .map_err(|error| {
                    AppCommandError::invalid_input(format!("Invalid directory entry: {error}"))
                })?;
            let entry_name = format!("{}/{}", name, relative.to_string_lossy().replace('\\', "/"));
            archive.start_file(entry_name, options).map_err(|error| {
                AppCommandError::io_error("Failed to create ZIP entry")
                    .with_detail(error.to_string())
            })?;
            let mut source = std::fs::File::open(entry.path()).map_err(AppCommandError::io)?;
            std::io::copy(&mut source, &mut archive).map_err(AppCommandError::io)?;
        }
    }
    let file = archive.finish().map_err(|error| {
        AppCommandError::io_error("Failed to finalize deliverables ZIP")
            .with_detail(error.to_string())
    })?;
    file.metadata()
        .map(|meta| meta.len())
        .map_err(AppCommandError::io)
}

pub async fn save_deliverables_core(
    conn: &DatabaseConnection,
    request: DeliverableDownloadRequest,
    destination: PathBuf,
) -> Result<DeliverableSaveResult, AppCommandError> {
    validate_ids(&request.deliverable_ids)?;
    if !destination.is_absolute() {
        return Err(AppCommandError::invalid_input(
            "The destination path must be absolute",
        ));
    }
    let resolved = deliverable_service::resolve_for_access(
        conn,
        request.conversation_id,
        &request.deliverable_ids,
    )
    .await
    .map_err(map_db_error)?;
    let archive = request.archive || resolved.len() != 1 || resolved[0].model.kind == "directory";
    let destination_for_task = destination.clone();
    let bytes = tokio::task::spawn_blocking(move || {
        if archive {
            write_deliverables_zip(&resolved, &destination_for_task)
        } else {
            std::fs::copy(&resolved[0].absolute_path, &destination_for_task)
                .map_err(AppCommandError::io)
        }
    })
    .await
    .map_err(|error| {
        AppCommandError::task_execution_failed("Deliverable save task failed")
            .with_detail(error.to_string())
    })??;
    Ok(DeliverableSaveResult {
        saved_path: destination.to_string_lossy().to_string(),
        bytes,
    })
}

#[cfg(target_os = "windows")]
async fn copy_paths_to_host_clipboard(paths: Vec<PathBuf>) -> Result<(), AppCommandError> {
    let (send, receive) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("codeg-file-clipboard-sta".into())
        .spawn(move || {
            let _ = send.send(copy_paths_to_windows_clipboard(&paths));
        })
        .map_err(AppCommandError::io)?;
    receive.await.map_err(|error| {
        AppCommandError::task_execution_failed("Clipboard STA thread stopped")
            .with_detail(error.to_string())
    })?
}

#[cfg(not(target_os = "windows"))]
async fn copy_paths_to_host_clipboard(_paths: Vec<PathBuf>) -> Result<(), AppCommandError> {
    Err(AppCommandError::configuration_invalid(
        "Copying file objects is supported only when Codeg Server runs on Windows",
    ))
}

#[cfg(target_os = "windows")]
fn copy_paths_to_windows_clipboard(paths: &[PathBuf]) -> Result<(), AppCommandError> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{copy_nonoverlapping, null_mut};
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows_sys::Win32::System::Ole::CF_HDROP;
    use windows_sys::Win32::UI::Shell::DROPFILES;

    let mut wide = Vec::<u16>::new();
    for path in paths {
        wide.extend(path.as_os_str().encode_wide());
        wide.push(0);
    }
    wide.push(0);
    let byte_len = size_of::<DROPFILES>() + wide.len() * size_of::<u16>();
    unsafe {
        let hr = CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED as u32);
        if hr < 0 {
            return Err(AppCommandError::task_execution_failed(
                "Failed to initialize the Windows clipboard STA thread",
            ));
        }
        let handle = GlobalAlloc(GMEM_MOVEABLE, byte_len);
        if handle.is_null() {
            CoUninitialize();
            return Err(AppCommandError::io_error(
                "Failed to allocate Windows clipboard memory",
            ));
        }
        let memory = GlobalLock(handle);
        if memory.is_null() {
            GlobalFree(handle);
            CoUninitialize();
            return Err(AppCommandError::io_error(
                "Failed to lock Windows clipboard memory",
            ));
        }
        let mut drop_files: DROPFILES = std::mem::zeroed();
        drop_files.pFiles = size_of::<DROPFILES>() as u32;
        drop_files.fWide = 1;
        copy_nonoverlapping(
            &drop_files as *const DROPFILES as *const u8,
            memory as *mut u8,
            size_of::<DROPFILES>(),
        );
        copy_nonoverlapping(
            wide.as_ptr() as *const u8,
            (memory as *mut u8).add(size_of::<DROPFILES>()),
            wide.len() * size_of::<u16>(),
        );
        GlobalUnlock(handle);

        let mut opened = false;
        for _ in 0..8 {
            if OpenClipboard(null_mut()) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        if !opened {
            GlobalFree(handle);
            CoUninitialize();
            return Err(AppCommandError::task_execution_failed(
                "The Windows clipboard is busy; try again",
            ));
        }
        if EmptyClipboard() == 0 || SetClipboardData(CF_HDROP as u32, handle).is_null() {
            CloseClipboard();
            GlobalFree(handle);
            CoUninitialize();
            return Err(AppCommandError::task_execution_failed(
                "Failed to place files on the Windows clipboard",
            ));
        }
        // SetClipboardData transfers ownership of `handle` to Windows.
        CloseClipboard();
        CoUninitialize();
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn reveal_path_on_host(path: &Path) -> Result<(), AppCommandError> {
    let mut select_argument = std::ffi::OsString::from("/select,");
    select_argument.push(path);
    std::process::Command::new("explorer.exe")
        .arg(select_argument)
        .spawn()
        .map_err(|error| {
            AppCommandError::external_command(
                "Failed to reveal deliverable in Windows Explorer",
                error.to_string(),
            )
        })?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn reveal_path_on_host(_path: &Path) -> Result<(), AppCommandError> {
    Err(AppCommandError::configuration_invalid(
        "Reveal in folder is supported only when Codeg Server runs on Windows",
    ))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn can_open_with_default_app() -> bool {
    true
}

#[cfg(target_os = "linux")]
fn can_open_with_default_app() -> bool {
    let has_graphical_session = std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("WSL_DISTRO_NAME").is_some();
    has_graphical_session
        && ["xdg-open", "gio", "gnome-open", "kde-open", "wslview"]
            .iter()
            .any(|launcher| which::which(launcher).is_ok())
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn can_open_with_default_app() -> bool {
    false
}

fn open_path_with_default_app(path: &Path) -> Result<(), AppCommandError> {
    if !can_open_with_default_app() {
        return Err(AppCommandError::configuration_invalid(
            "No graphical default-application launcher is available on the Codeg host",
        ));
    }
    open::that_detached(path).map_err(|error| {
        AppCommandError::external_command(
            "Failed to open deliverable with its default application",
            error.to_string(),
        )
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub fn deliverable_capabilities() -> DeliverableCapabilities {
    deliverable_capabilities_core()
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_conversation_deliverables(
    conversation_id: i32,
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<Vec<ConversationDeliverable>, AppCommandError> {
    list_conversation_deliverables_core(&db.conn, conversation_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_turn_deliverables(
    conversation_id: i32,
    turn_run_id: String,
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<Vec<ConversationDeliverable>, AppCommandError> {
    list_turn_deliverables_core(&db.conn, conversation_id, &turn_run_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_conversation_deliverable_runs(
    conversation_id: i32,
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<Vec<ConversationTurnDeliverableSet>, AppCommandError> {
    list_conversation_deliverable_runs_core(&db.conn, conversation_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn copy_deliverables(
    conversation_id: i32,
    deliverable_ids: Vec<String>,
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<DeliverableOperationResult, AppCommandError> {
    copy_deliverables_core(
        &db.conn,
        DeliverableIdsRequest {
            conversation_id,
            deliverable_ids,
        },
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn reveal_deliverable(
    conversation_id: i32,
    deliverable_id: String,
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<DeliverableOperationResult, AppCommandError> {
    reveal_deliverable_core(&db.conn, conversation_id, deliverable_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_deliverable(
    conversation_id: i32,
    deliverable_id: String,
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<DeliverableOperationResult, AppCommandError> {
    open_deliverable_core(&db.conn, conversation_id, deliverable_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn hide_deliverables(
    conversation_id: i32,
    deliverable_ids: Vec<String>,
    db: tauri::State<'_, crate::db::AppDatabase>,
    app: tauri::AppHandle,
) -> Result<DeliverableOperationResult, AppCommandError> {
    let result = hide_deliverables_core(
        &db.conn,
        DeliverableIdsRequest {
            conversation_id,
            deliverable_ids,
        },
    )
    .await?;
    crate::acp::deliverables::emit_deliverables_changed(
        &crate::web::event_bridge::EventEmitter::Tauri(app),
        conversation_id,
        Vec::new(),
    );
    Ok(result)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn save_deliverables(
    conversation_id: i32,
    deliverable_ids: Vec<String>,
    archive: bool,
    destination: String,
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<DeliverableSaveResult, AppCommandError> {
    save_deliverables_core(
        &db.conn,
        DeliverableDownloadRequest {
            conversation_id,
            deliverable_ids,
            archive,
        },
        PathBuf::from(destination),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, name: &str, absolute_path: PathBuf) -> ResolvedDeliverable {
        use chrono::Utc;
        ResolvedDeliverable {
            model: crate::db::entities::conversation_deliverable::Model {
                id: id.into(),
                conversation_id: 1,
                turn_run_id: None,
                root_path: absolute_path
                    .parent()
                    .unwrap_or(Path::new("/tmp"))
                    .to_string_lossy()
                    .to_string(),
                path: name.into(),
                kind: "file".into(),
                title: name.into(),
                description: None,
                role: "primary".into(),
                position: 0,
                source: "declared".into(),
                file_name: name.into(),
                extension: Path::new(name)
                    .extension()
                    .map(|value| value.to_string_lossy().to_string()),
                size_bytes: Some(1),
                modified_at: None,
                is_valid: true,
                invalid_reason: None,
                is_hidden: false,
                verified_at: Utc::now(),
                last_checked_at: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            absolute_path,
        }
    }

    #[test]
    fn duplicate_archive_names_are_predictable_and_keep_unicode() {
        let names = predictable_archive_names(&[
            item("1", "中文报告.pdf", PathBuf::from("/tmp/a.pdf")),
            item("2", "中文报告.pdf", PathBuf::from("/tmp/b.pdf")),
            item("3", "中文报告.pdf", PathBuf::from("/tmp/c.pdf")),
        ]);
        assert_eq!(
            names,
            ["中文报告.pdf", "中文报告 (2).pdf", "中文报告 (3).pdf"]
        );
    }

    #[test]
    fn zip_keeps_unicode_names_contents_and_predictable_duplicates() {
        let source = tempfile::tempdir().unwrap();
        let first = source.path().join("first.pdf");
        let second = source.path().join("second.pdf");
        std::fs::write(&first, b"first payload").unwrap();
        std::fs::write(&second, b"second payload").unwrap();
        let output = source.path().join("bundle.zip");
        let items = [
            item("1", "中文 报告 (最终).pdf", first),
            item("2", "中文 报告 (最终).pdf", second),
        ];

        assert!(write_deliverables_zip(&items, &output).unwrap() > 0);
        let file = std::fs::File::open(output).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert_eq!(archive.len(), 2);
        let mut first_payload = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("中文 报告 (最终).pdf").unwrap(),
            &mut first_payload,
        )
        .unwrap();
        assert_eq!(first_payload, "first payload");
        let mut second_payload = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("中文 报告 (最终) (2).pdf").unwrap(),
            &mut second_payload,
        )
        .unwrap();
        assert_eq!(second_payload, "second payload");
    }

    #[tokio::test]
    async fn single_file_response_streams_unicode_name_length_mime_and_body() {
        let source = tempfile::tempdir().unwrap();
        let name = "中文 报告 (最终).docx";
        let path = source.path().join(name);
        std::fs::write(&path, b"document payload").unwrap();

        let response = crate::web::handlers::workspace_files::stream_file_response(&path, name)
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
        assert_eq!(response.headers()[axum::http::header::CONTENT_LENGTH], "16");
        let disposition = response.headers()[axum::http::header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap();
        assert!(disposition.contains("filename*=UTF-8''"));
        assert!(disposition.contains("%E4%B8%AD%E6%96%87"));
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), b"document payload");
    }

    #[test]
    fn common_deliverable_extensions_have_download_mime_types() {
        for (name, expected) in [
            (
                "report.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            (
                "slides.pptx",
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            ),
            (
                "sheet.xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ),
            ("document.pdf", "application/pdf"),
            ("image.png", "image/png"),
            ("bundle.zip", "application/zip"),
        ] {
            assert_eq!(
                mime_guess::from_path(name).first_or_octet_stream().as_ref(),
                expected
            );
        }
    }

    #[test]
    fn capabilities_serialize_the_default_application_flag() {
        let value = serde_json::to_value(deliverable_capabilities_core()).unwrap();
        assert!(value.get("openWithDefaultApp").is_some());
    }
}
