use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::error::AppError;
use crate::state::WebState;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionRequest {
    connection_id: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContainerListRequest {
    connection_id: String,
    #[serde(default = "default_true")]
    all: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContainerRequest {
    connection_id: String,
    container_id: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContainerActionRequest {
    connection_id: String,
    container_id: String,
    action: dbx_core::docker::DockerContainerAction,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContainerStatsRequest {
    connection_id: String,
    container_ids: Vec<String>,
}

fn default_true() -> bool {
    true
}

pub async fn test_connection(
    State(state): State<Arc<WebState>>,
    Json(request): Json<ConnectionRequest>,
) -> Result<Json<dbx_core::docker::DockerConnectionInfo>, AppError> {
    Ok(Json(
        dbx_core::docker::docker_test_connection_core(&state.app, &request.connection_id)
            .await
            .map_err(AppError::from)?,
    ))
}

pub async fn list_containers(
    State(state): State<Arc<WebState>>,
    Json(request): Json<ContainerListRequest>,
) -> Result<Json<Vec<dbx_core::docker::DockerContainer>>, AppError> {
    Ok(Json(
        dbx_core::docker::docker_list_containers_core(&state.app, &request.connection_id, request.all)
            .await
            .map_err(AppError::from)?,
    ))
}

pub async fn list_images(
    State(state): State<Arc<WebState>>,
    Json(request): Json<ConnectionRequest>,
) -> Result<Json<Vec<dbx_core::docker::DockerImage>>, AppError> {
    Ok(Json(
        dbx_core::docker::docker_list_images_core(&state.app, &request.connection_id).await.map_err(AppError::from)?,
    ))
}

pub async fn list_volumes(
    State(state): State<Arc<WebState>>,
    Json(request): Json<ConnectionRequest>,
) -> Result<Json<Vec<dbx_core::docker::DockerVolume>>, AppError> {
    Ok(Json(
        dbx_core::docker::docker_list_volumes_core(&state.app, &request.connection_id).await.map_err(AppError::from)?,
    ))
}

pub async fn list_networks(
    State(state): State<Arc<WebState>>,
    Json(request): Json<ConnectionRequest>,
) -> Result<Json<Vec<dbx_core::docker::DockerNetwork>>, AppError> {
    Ok(Json(
        dbx_core::docker::docker_list_networks_core(&state.app, &request.connection_id)
            .await
            .map_err(AppError::from)?,
    ))
}

pub async fn container_action(
    State(state): State<Arc<WebState>>,
    Json(request): Json<ContainerActionRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if state.password_disabled {
        return Err(AppError {
            message: "Docker lifecycle operations are disabled when DBX password protection is disabled".to_string(),
            status: StatusCode::FORBIDDEN,
        });
    }
    dbx_core::docker::docker_container_action_core(
        &state.app,
        &request.connection_id,
        &request.container_id,
        request.action,
    )
    .await
    .map_err(AppError::from)?;
    Ok(Json(serde_json::json!({"ok": true})))
}

pub async fn inspect_container(
    State(state): State<Arc<WebState>>,
    Json(request): Json<ContainerRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(
        dbx_core::docker::docker_inspect_container_core(&state.app, &request.connection_id, &request.container_id)
            .await
            .map_err(AppError::from)?,
    ))
}

pub async fn container_stats(
    State(state): State<Arc<WebState>>,
    Json(request): Json<ContainerStatsRequest>,
) -> Result<Json<Vec<dbx_core::docker::DockerContainerStats>>, AppError> {
    Ok(Json(
        dbx_core::docker::docker_container_stats_core(&state.app, &request.connection_id, request.container_ids)
            .await
            .map_err(AppError::from)?,
    ))
}
