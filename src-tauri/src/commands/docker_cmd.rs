use std::sync::Arc;

use tauri::State;

use crate::commands::connection::AppState;

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
