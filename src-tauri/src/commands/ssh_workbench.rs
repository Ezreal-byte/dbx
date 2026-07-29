use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use dbx_core::connection::AppState;
use dbx_core::models::connection::ConnectionConfig;
use dbx_core::ssh_workbench::{SftpEntry, SftpTransferTask, SshSessionInfo};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpPreview {
    pub base64: String,
    pub size: usize,
}

#[tauri::command]
pub async fn ssh_test_connection(state: State<'_, Arc<AppState>>, config: ConnectionConfig) -> Result<String, String> {
    state.ssh_registry.test_connection(&config).await?;
    Ok("SSH connection successful".to_string())
}

#[tauri::command]
pub async fn ssh_create_session(
    state: State<'_, Arc<AppState>>,
    mut config: ConnectionConfig,
    cols: u32,
    rows: u32,
) -> Result<SshSessionInfo, String> {
    if let Some(stored) = state.configs.read().await.get(&config.id) {
        config.read_only = stored.read_only;
    }
    state.ssh_registry.create_session(&config, cols, rows).await
}

#[tauri::command]
pub async fn ssh_close_session(state: State<'_, Arc<AppState>>, session_id: String) -> Result<(), String> {
    state.ssh_registry.close_session(&session_id).await
}

#[tauri::command]
pub fn ssh_terminal_server_port(
    port: tauri::State<'_, super::redis_pubsub_server::PubSubServerPort>,
) -> Result<u16, String> {
    port.ssh_get()
}

#[tauri::command]
pub async fn sftp_home(state: State<'_, Arc<AppState>>, session_id: String) -> Result<String, String> {
    state.ssh_registry.sftp_home(&session_id).await
}

#[tauri::command]
pub async fn sftp_list(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
) -> Result<Vec<SftpEntry>, String> {
    state.ssh_registry.sftp_list(&session_id, &path).await
}

#[tauri::command]
pub async fn sftp_mkdir(state: State<'_, Arc<AppState>>, session_id: String, path: String) -> Result<(), String> {
    state.ssh_registry.sftp_mkdir(&session_id, &path).await
}

#[tauri::command]
pub async fn sftp_rename(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    from: String,
    to: String,
) -> Result<(), String> {
    state.ssh_registry.sftp_rename(&session_id, &from, &to).await
}

#[tauri::command]
pub async fn sftp_delete(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
    recursive: bool,
) -> Result<(), String> {
    state.ssh_registry.sftp_delete(&session_id, &path, recursive).await
}

#[tauri::command]
pub async fn sftp_preview(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
    max_bytes: usize,
) -> Result<SftpPreview, String> {
    let bytes = state.ssh_registry.sftp_read(&session_id, &path, max_bytes).await?;
    Ok(SftpPreview { size: bytes.len(), base64: base64::engine::general_purpose::STANDARD.encode(bytes) })
}

#[tauri::command]
pub async fn sftp_upload(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    task_id: String,
    local_path: String,
    remote_path: String,
) -> Result<SftpTransferTask, String> {
    state.ssh_registry.sftp_upload_task(&session_id, &task_id, &PathBuf::from(local_path), &remote_path).await
}

#[tauri::command]
pub async fn sftp_download(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    task_id: String,
    remote_path: String,
    local_path: String,
) -> Result<SftpTransferTask, String> {
    state.ssh_registry.sftp_download_task(&session_id, &task_id, &remote_path, &PathBuf::from(local_path)).await
}

#[tauri::command]
pub async fn listen_sftp_transfer_progress(
    state: State<'_, Arc<AppState>>,
    on_progress: Channel<SftpTransferTask>,
) -> Result<(), String> {
    let mut receiver = state.ssh_registry.subscribe_transfer_progress();
    tokio::spawn(async move {
        while let Ok(progress) = receiver.recv().await {
            if on_progress.send(progress).is_err() {
                break;
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn cancel_sftp_transfer(state: State<'_, Arc<AppState>>, task_id: String) -> Result<(), String> {
    state.ssh_registry.cancel_sftp_transfer(&task_id).await
}
