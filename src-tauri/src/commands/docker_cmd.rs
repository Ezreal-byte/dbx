use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

use futures::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{watch, Mutex};

use crate::commands::connection::AppState;

static DOCKER_STREAMS: OnceLock<Mutex<HashMap<String, watch::Sender<bool>>>> = OnceLock::new();

fn docker_streams() -> &'static Mutex<HashMap<String, watch::Sender<bool>>> {
    DOCKER_STREAMS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DockerStreamEvent {
    session_id: String,
    chunk: String,
    done: bool,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DockerTransferProgress {
    session_id: String,
    kind: String,
    direction: String,
    image: String,
    status: String,
    bytes_completed: u64,
    bytes_total: Option<u64>,
    layers_completed: Option<u64>,
    layers_total: Option<u64>,
    message: Option<String>,
    error: Option<String>,
}

fn emit_transfer(
    app: &AppHandle,
    session_id: &str,
    kind: &str,
    direction: &str,
    image: &str,
    status: &str,
    bytes_completed: u64,
    bytes_total: Option<u64>,
    message: Option<String>,
    error: Option<String>,
) {
    let _ = app.emit(
        "docker-transfer-progress",
        DockerTransferProgress {
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            direction: direction.to_string(),
            image: image.to_string(),
            status: status.to_string(),
            bytes_completed,
            bytes_total,
            layers_completed: None,
            layers_total: None,
            message,
            error,
        },
    );
}

fn docker_json_stream_error(chunk: &[u8]) -> Option<String> {
    String::from_utf8_lossy(chunk)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|value| {
            value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    value
                        .get("errorDetail")
                        .and_then(|detail| detail.get("message"))
                        .and_then(serde_json::Value::as_str)
                })
                .map(str::to_string)
        })
}

#[tauri::command]
pub async fn docker_test_connection(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
) -> Result<dbx_core::docker::DockerConnectionInfo, String> {
    dbx_core::docker::docker_test_connection_core(&state, &connection_id).await
}

#[tauri::command]
pub async fn docker_get_engine_details(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
) -> Result<dbx_core::docker::DockerEngineDetails, String> {
    dbx_core::docker::docker_get_engine_details_core(&state, &connection_id).await
}

#[tauri::command]
pub async fn docker_list_containers(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    all: bool,
) -> Result<Vec<dbx_core::docker::DockerContainer>, String> {
    dbx_core::docker::docker_list_containers_core(&state, &connection_id, all).await
}

#[tauri::command]
pub async fn docker_list_images(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
) -> Result<Vec<dbx_core::docker::DockerImage>, String> {
    dbx_core::docker::docker_list_images_core(&state, &connection_id).await
}

#[tauri::command]
pub async fn docker_list_volumes(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
) -> Result<Vec<dbx_core::docker::DockerVolume>, String> {
    dbx_core::docker::docker_list_volumes_core(&state, &connection_id).await
}

#[tauri::command]
pub async fn docker_list_networks(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
) -> Result<Vec<dbx_core::docker::DockerNetwork>, String> {
    dbx_core::docker::docker_list_networks_core(&state, &connection_id).await
}

#[tauri::command]
pub async fn docker_container_action(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    container_id: String,
    action: dbx_core::docker::DockerContainerAction,
) -> Result<(), String> {
    dbx_core::docker::docker_container_action_core(&state, &connection_id, &container_id, action).await
}

#[tauri::command]
pub async fn docker_inspect_container(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    container_id: String,
) -> Result<serde_json::Value, String> {
    dbx_core::docker::docker_inspect_container_core(&state, &connection_id, &container_id).await
}

#[tauri::command]
pub async fn docker_container_stats(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    container_ids: Vec<String>,
) -> Result<Vec<dbx_core::docker::DockerContainerStats>, String> {
    dbx_core::docker::docker_container_stats_core(&state, &connection_id, container_ids).await
}

#[tauri::command]
pub async fn docker_create_container(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    request: dbx_core::docker::DockerCreateContainerRequest,
) -> Result<dbx_core::docker::DockerCreateContainerResult, String> {
    dbx_core::docker::docker_create_container_core(&state, &connection_id, request).await
}

#[tauri::command]
pub async fn docker_apply_compose(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    request: dbx_core::docker::DockerComposeApplyRequest,
) -> Result<dbx_core::docker::DockerComposeApplyResult, String> {
    dbx_core::docker::docker_apply_compose_core(&state, &connection_id, request).await
}

#[tauri::command]
pub async fn docker_remove_container(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    container_id: String,
) -> Result<(), String> {
    dbx_core::docker::docker_remove_container_core(&state, &connection_id, &container_id).await
}

#[tauri::command]
pub async fn docker_remove_image(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    image_id: String,
) -> Result<(), String> {
    dbx_core::docker::docker_remove_image_core(&state, &connection_id, &image_id).await
}

#[tauri::command]
pub async fn docker_create_volume(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    request: dbx_core::docker::DockerCreateVolumeRequest,
) -> Result<dbx_core::docker::DockerVolume, String> {
    dbx_core::docker::docker_create_volume_core(&state, &connection_id, request).await
}

#[tauri::command]
pub async fn docker_create_network(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    request: dbx_core::docker::DockerCreateNetworkRequest,
) -> Result<dbx_core::docker::DockerCreateNetworkResult, String> {
    dbx_core::docker::docker_create_network_core(&state, &connection_id, request).await
}

#[tauri::command]
pub async fn docker_list_container_files(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    container_id: String,
    path: String,
) -> Result<Vec<dbx_core::docker::DockerFileEntry>, String> {
    dbx_core::docker::docker_list_container_files_core(&state, &connection_id, &container_id, &path).await
}

#[tauri::command]
pub async fn docker_preview_container_file(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    container_id: String,
    path: String,
) -> Result<dbx_core::docker::DockerFilePreview, String> {
    dbx_core::docker::docker_preview_container_file_core(&state, &connection_id, &container_id, &path).await
}

#[tauri::command]
pub async fn docker_download_container_file(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    container_id: String,
    path: String,
) -> Result<Vec<u8>, String> {
    dbx_core::docker::docker_download_container_file_core(&state, &connection_id, &container_id, &path).await
}

#[tauri::command]
pub async fn docker_export_image(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    image_id: String,
) -> Result<Vec<u8>, String> {
    dbx_core::docker::docker_export_image_bytes_core(&state, &connection_id, &image_id).await
}

#[tauri::command]
pub async fn docker_export_image_to_path(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    image_id: String,
    destination_path: String,
) -> Result<u64, String> {
    dbx_core::docker::docker_export_image_to_path_core(&state, &connection_id, &image_id, &destination_path).await
}

#[tauri::command]
pub async fn docker_start_image_export(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    connection_id: String,
    image_id: String,
    destination_path: String,
) -> Result<(), String> {
    if destination_path.trim().is_empty() {
        return Err("Image export destination is required".to_string());
    }
    let (cancel_sender, mut cancelled) = watch::channel(false);
    docker_streams().lock().await.insert(session_id.clone(), cancel_sender);
    let app_state = state.inner().clone();
    let task_session_id = session_id.clone();
    tauri::async_runtime::spawn(async move {
        let destination = std::path::PathBuf::from(&destination_path);
        let mut written = 0u64;
        let mut bytes_total = None;
        let result = async {
            let response =
                dbx_core::docker::docker_export_image_response_core(&app_state, &connection_id, &image_id).await?;
            let total = response.content_length();
            bytes_total = total;
            emit_transfer(&app, &task_session_id, "export", "download", &image_id, "running", 0, total, None, None);
            let mut stream = response.bytes_stream();
            let mut file = tokio::fs::File::create(&destination)
                .await
                .map_err(|error| format!("Failed to create image export file: {error}"))?;
            loop {
                let chunk = tokio::select! {
                    changed = cancelled.changed() => {
                        if changed.is_err() || *cancelled.borrow() {
                            return Err("__cancelled__".to_string());
                        }
                        continue;
                    }
                    chunk = stream.next() => {
                        let Some(chunk) = chunk else { break; };
                        chunk
                    }
                };
                let chunk = chunk.map_err(|error| format!("Docker image export failed: {error}"))?;
                file.write_all(&chunk).await.map_err(|error| format!("Failed to write image export: {error}"))?;
                written += chunk.len() as u64;
                emit_transfer(
                    &app,
                    &task_session_id,
                    "export",
                    "download",
                    &image_id,
                    "running",
                    written,
                    total,
                    None,
                    None,
                );
            }
            file.flush().await.map_err(|error| format!("Failed to finish image export: {error}"))?;
            Ok::<(), String>(())
        }
        .await;
        let (status, error) = match result {
            Ok(()) => ("done", None),
            Err(error) if error == "__cancelled__" => {
                let _ = tokio::fs::remove_file(&destination).await;
                ("cancelled", None)
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&destination).await;
                ("error", Some(error))
            }
        };
        emit_transfer(
            &app,
            &task_session_id,
            "export",
            "download",
            &image_id,
            status,
            written,
            bytes_total,
            None,
            error,
        );
        docker_streams().lock().await.remove(&task_session_id);
    });
    Ok(())
}

#[tauri::command]
pub async fn docker_start_logs(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    connection_id: String,
    container_id: String,
    options: dbx_core::docker::DockerLogOptions,
) -> Result<(), String> {
    let (cancel_sender, mut cancelled) = watch::channel(false);
    docker_streams().lock().await.insert(session_id.clone(), cancel_sender);
    let app_state = state.inner().clone();
    let task_session_id = session_id.clone();
    tauri::async_runtime::spawn(async move {
        let result = async {
            let response = dbx_core::docker::docker_container_logs_response_core(
                &app_state,
                &connection_id,
                &container_id,
                options,
            )
            .await?;
            let mut stream = response.bytes_stream();
            let mut frame_buffer = Vec::new();
            loop {
                let chunk = tokio::select! {
                    changed = cancelled.changed() => {
                        if changed.is_err() || *cancelled.borrow() {
                            break;
                        }
                        continue;
                    }
                    chunk = stream.next() => {
                        let Some(chunk) = chunk else {
                            break;
                        };
                        chunk
                    }
                };
                let chunk = chunk.map_err(|error| format!("Docker log stream failed: {error}"))?;
                let decoded = dbx_core::docker::decode_multiplexed_stream_chunk(&mut frame_buffer, &chunk);
                if !decoded.is_empty() {
                    let _ = app.emit(
                        "docker-log-stream",
                        DockerStreamEvent {
                            session_id: task_session_id.clone(),
                            chunk: String::from_utf8_lossy(&decoded).into_owned(),
                            done: false,
                            error: None,
                        },
                    );
                }
            }
            Ok::<(), String>(())
        }
        .await;
        let error = result.err();
        let _ = app.emit(
            "docker-log-stream",
            DockerStreamEvent { session_id: task_session_id.clone(), chunk: String::new(), done: true, error },
        );
        docker_streams().lock().await.remove(&task_session_id);
    });
    Ok(())
}

#[tauri::command]
pub async fn docker_pull_image(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    connection_id: String,
    image: String,
    auth: Option<dbx_core::docker::DockerRegistryAuth>,
) -> Result<(), String> {
    let (cancel_sender, mut cancelled) = watch::channel(false);
    docker_streams().lock().await.insert(session_id.clone(), cancel_sender);
    let app_state = state.inner().clone();
    let task_session_id = session_id.clone();
    tauri::async_runtime::spawn(async move {
        let mut transferred = 0u64;
        let mut was_cancelled = false;
        emit_transfer(&app, &task_session_id, "pull", "download", &image, "running", 0, None, None, None);
        let result = async {
            let response =
                dbx_core::docker::docker_pull_image_response_core(&app_state, &connection_id, &image, auth).await?;
            let mut stream = response.bytes_stream();
            loop {
                let chunk = tokio::select! {
                    changed = cancelled.changed() => {
                        if changed.is_err() || *cancelled.borrow() {
                            was_cancelled = true;
                            break;
                        }
                        continue;
                    }
                    chunk = stream.next() => {
                        let Some(chunk) = chunk else {
                            break;
                        };
                        chunk
                    }
                };
                let chunk = chunk.map_err(|error| format!("Docker image pull failed: {error}"))?;
                if let Some(error) = docker_json_stream_error(&chunk) {
                    return Err(format!("Docker image pull failed: {error}"));
                }
                transferred += chunk.len() as u64;
                emit_transfer(
                    &app,
                    &task_session_id,
                    "pull",
                    "download",
                    &image,
                    "running",
                    transferred,
                    None,
                    Some(String::from_utf8_lossy(&chunk).into_owned()),
                    None,
                );
                let _ = app.emit(
                    "docker-image-pull",
                    DockerStreamEvent {
                        session_id: task_session_id.clone(),
                        chunk: String::from_utf8_lossy(&chunk).into_owned(),
                        done: false,
                        error: None,
                    },
                );
            }
            Ok::<(), String>(())
        }
        .await;
        let error = result.err();
        emit_transfer(
            &app,
            &task_session_id,
            "pull",
            "download",
            &image,
            if was_cancelled {
                "cancelled"
            } else if error.is_some() {
                "error"
            } else {
                "done"
            },
            transferred,
            None,
            None,
            error.clone(),
        );
        let _ = app.emit(
            "docker-image-pull",
            DockerStreamEvent { session_id: task_session_id.clone(), chunk: String::new(), done: true, error },
        );
        docker_streams().lock().await.remove(&task_session_id);
    });
    Ok(())
}

#[tauri::command]
pub async fn docker_push_image(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    connection_id: String,
    source_image_id: String,
    target_reference: String,
    auth: Option<dbx_core::docker::DockerRegistryAuth>,
) -> Result<(), String> {
    let (cancel_sender, mut cancelled) = watch::channel(false);
    docker_streams().lock().await.insert(session_id.clone(), cancel_sender);
    let app_state = state.inner().clone();
    let task_session_id = session_id.clone();
    tauri::async_runtime::spawn(async move {
        emit_transfer(&app, &task_session_id, "push", "upload", &target_reference, "running", 0, None, None, None);
        let result = async {
            let response = dbx_core::docker::docker_push_image_response_core(
                &app_state,
                &connection_id,
                &source_image_id,
                &target_reference,
                auth,
            )
            .await?;
            let mut stream = response.bytes_stream();
            let mut transferred = 0u64;
            loop {
                let chunk = tokio::select! {
                    changed = cancelled.changed() => {
                        if changed.is_err() || *cancelled.borrow() {
                            return Err("__cancelled__".to_string());
                        }
                        continue;
                    }
                    chunk = stream.next() => {
                        let Some(chunk) = chunk else { break; };
                        chunk
                    }
                };
                let chunk = chunk.map_err(|error| format!("Docker image push failed: {error}"))?;
                if let Some(error) = docker_json_stream_error(&chunk) {
                    return Err(format!("Docker image push failed: {error}"));
                }
                transferred += chunk.len() as u64;
                emit_transfer(
                    &app,
                    &task_session_id,
                    "push",
                    "upload",
                    &target_reference,
                    "running",
                    transferred,
                    None,
                    Some(String::from_utf8_lossy(&chunk).into_owned()),
                    None,
                );
            }
            Ok::<u64, String>(transferred)
        }
        .await;
        let (status, bytes, error) = match result {
            Ok(bytes) => ("done", bytes, None),
            Err(error) if error == "__cancelled__" => ("cancelled", 0, None),
            Err(error) => ("error", 0, Some(error)),
        };
        emit_transfer(&app, &task_session_id, "push", "upload", &target_reference, status, bytes, None, None, error);
        docker_streams().lock().await.remove(&task_session_id);
    });
    Ok(())
}

#[tauri::command]
pub async fn docker_load_image_from_path(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    connection_id: String,
    source_path: String,
) -> Result<(), String> {
    let metadata = tokio::fs::metadata(&source_path)
        .await
        .map_err(|error| format!("Failed to read image archive metadata: {error}"))?;
    if !metadata.is_file() {
        return Err("Image archive must be a file".to_string());
    }
    let file =
        tokio::fs::File::open(&source_path).await.map_err(|error| format!("Failed to open image archive: {error}"))?;
    let image_name = std::path::Path::new(&source_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("docker-image.tar")
        .to_string();
    let total = metadata.len();
    let (cancel_sender, cancelled) = watch::channel(false);
    docker_streams().lock().await.insert(session_id.clone(), cancel_sender);
    let app_state = state.inner().clone();
    let task_session_id = session_id.clone();
    tauri::async_runtime::spawn(async move {
        let cancellation_observed = Arc::new(AtomicBool::new(false));
        let transferred_bytes = Arc::new(AtomicU64::new(0));
        emit_transfer(&app, &task_session_id, "load", "upload", &image_name, "running", 0, Some(total), None, None);
        let progress_app = app.clone();
        let progress_session = task_session_id.clone();
        let progress_image = image_name.clone();
        let progress_cancelled = cancellation_observed.clone();
        let progress_transferred = transferred_bytes.clone();
        let upload_stream = futures::stream::try_unfold(
            (file, cancelled, 0u64),
            move |(mut file, mut cancelled, transferred)| {
                let app = progress_app.clone();
                let session_id = progress_session.clone();
                let image = progress_image.clone();
                let cancellation_observed = progress_cancelled.clone();
                let transferred_bytes = progress_transferred.clone();
                async move {
                    let mut buffer = vec![0u8; 256 * 1024];
                    let read = tokio::select! {
                        changed = cancelled.changed() => {
                            if changed.is_err() || *cancelled.borrow() {
                                cancellation_observed.store(true, Ordering::Relaxed);
                                return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "Docker image load cancelled"));
                            }
                            0
                        }
                        read = file.read(&mut buffer) => read?
                    };
                    if read == 0 {
                        return Ok(None);
                    }
                    buffer.truncate(read);
                    let next_transferred = transferred + read as u64;
                    transferred_bytes.store(next_transferred, Ordering::Relaxed);
                    emit_transfer(
                        &app,
                        &session_id,
                        "load",
                        "upload",
                        &image,
                        "running",
                        next_transferred,
                        Some(total),
                        None,
                        None,
                    );
                    Ok(Some((buffer, (file, cancelled, next_transferred))))
                }
            },
        );
        let result = async {
            let response = dbx_core::docker::docker_load_image_response_core(
                &app_state,
                &connection_id,
                reqwest::Body::wrap_stream(upload_stream),
            )
            .await?;
            let bytes = response
                .bytes()
                .await
                .map_err(|error| format!("Failed to read Docker image load response: {error}"))?;
            if let Some(error) = docker_json_stream_error(&bytes) {
                return Err(format!("Docker image load failed: {error}"));
            }
            Ok::<(), String>(())
        }
        .await;
        let (status, error) = match result {
            Ok(()) => ("done", None),
            Err(_) if cancellation_observed.load(Ordering::Relaxed) => ("cancelled", None),
            Err(error) => ("error", Some(error)),
        };
        emit_transfer(
            &app,
            &task_session_id,
            "load",
            "upload",
            &image_name,
            status,
            transferred_bytes.load(Ordering::Relaxed),
            Some(total),
            None,
            error,
        );
        docker_streams().lock().await.remove(&task_session_id);
    });
    Ok(())
}

#[tauri::command]
pub async fn docker_stop_transfer(session_id: String) -> Result<bool, String> {
    docker_stop_stream(session_id).await
}

#[tauri::command]
pub async fn docker_stop_stream(session_id: String) -> Result<bool, String> {
    let cancelled = docker_streams().lock().await.remove(&session_id);
    if let Some(cancelled) = cancelled {
        let _ = cancelled.send(true);
        Ok(true)
    } else {
        Ok(false)
    }
}
