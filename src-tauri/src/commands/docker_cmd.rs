use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;

use futures::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
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

#[tauri::command]
pub async fn docker_test_connection(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
) -> Result<dbx_core::docker::DockerConnectionInfo, String> {
    dbx_core::docker::docker_test_connection_core(&state, &connection_id).await
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
        let result = async {
            let response =
                dbx_core::docker::docker_pull_image_response_core(&app_state, &connection_id, &image, auth).await?;
            let mut stream = response.bytes_stream();
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
                let chunk = chunk.map_err(|error| format!("Docker image pull failed: {error}"))?;
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
        let _ = app.emit(
            "docker-image-pull",
            DockerStreamEvent { session_id: task_session_id.clone(), chunk: String::new(), done: true, error },
        );
        docker_streams().lock().await.remove(&task_session_id);
    });
    Ok(())
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
