use std::sync::Arc;
use std::{convert::Infallible, time::Duration};

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Multipart, Query, State, WebSocketUpgrade};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use dbx_core::models::connection::ConnectionConfig;
use dbx_core::ssh_workbench::{SftpEntry, SftpTransferTask, SshSessionInfo, TerminalFrame, TerminalStream};

use crate::state::{SshWebDownload, WebState};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    config: ConnectionConfig,
    #[serde(default = "default_cols")]
    cols: u32,
    #[serde(default = "default_rows")]
    rows: u32,
}

fn default_cols() -> u32 {
    120
}

fn default_rows() -> u32 {
    32
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    session_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathRequest {
    session_id: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameRequest {
    session_id: String,
    from: String,
    to: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteRequest {
    session_id: String,
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRequest {
    session_id: String,
    path: String,
    #[serde(default = "default_preview_limit")]
    max_bytes: usize,
}

fn default_preview_limit() -> usize {
    2 * 1024 * 1024
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResponse {
    base64: String,
    size: usize,
}

pub async fn test_connection(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(config): Json<ConnectionConfig>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let _ = request_owner(&state, &headers).await?;
    state.app.ssh_registry.test_connection(&config).await.map_err(internal_error)?;
    Ok(Json(serde_json::json!({ "message": "SSH connection successful" })))
}

pub async fn create_session(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<CreateSessionRequest>,
) -> Result<Json<SshSessionInfo>, (StatusCode, String)> {
    let owner_session = request_owner(&state, &headers).await?;
    let mut config = request.config;
    if let Some(stored) = state.app.configs.read().await.get(&config.id) {
        config.read_only = stored.read_only;
    }
    state
        .app
        .ssh_registry
        .create_session_owned(&config, request.cols, request.rows, Some(owner_session))
        .await
        .map(Json)
        .map_err(internal_error)
}

pub async fn close_session(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<SessionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    authorize_session(&state, &headers, &request.session_id).await?;
    state.app.ssh_registry.close_session(&request.session_id).await.map_err(internal_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn sftp_home(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<SessionRequest>,
) -> Result<Json<String>, (StatusCode, String)> {
    authorize_session(&state, &headers, &request.session_id).await?;
    state.app.ssh_registry.sftp_home(&request.session_id).await.map(Json).map_err(internal_error)
}

pub async fn sftp_list(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<PathRequest>,
) -> Result<Json<Vec<SftpEntry>>, (StatusCode, String)> {
    authorize_session(&state, &headers, &request.session_id).await?;
    state.app.ssh_registry.sftp_list(&request.session_id, &request.path).await.map(Json).map_err(internal_error)
}

pub async fn sftp_mkdir(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<PathRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    authorize_session(&state, &headers, &request.session_id).await?;
    state.app.ssh_registry.sftp_mkdir(&request.session_id, &request.path).await.map_err(internal_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn sftp_rename(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<RenameRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    authorize_session(&state, &headers, &request.session_id).await?;
    state
        .app
        .ssh_registry
        .sftp_rename(&request.session_id, &request.from, &request.to)
        .await
        .map_err(internal_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn sftp_delete(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<DeleteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    authorize_session(&state, &headers, &request.session_id).await?;
    state
        .app
        .ssh_registry
        .sftp_delete(&request.session_id, &request.path, request.recursive)
        .await
        .map_err(internal_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn sftp_preview(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<PreviewRequest>,
) -> Result<Json<PreviewResponse>, (StatusCode, String)> {
    authorize_session(&state, &headers, &request.session_id).await?;
    let bytes = state
        .app
        .ssh_registry
        .sftp_read(&request.session_id, &request.path, request.max_bytes.min(default_preview_limit()))
        .await
        .map_err(internal_error)?;
    Ok(Json(PreviewResponse { size: bytes.len(), base64: base64::engine::general_purpose::STANDARD.encode(bytes) }))
}

pub async fn sftp_upload(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<SftpTransferTask>, (StatusCode, String)> {
    let mut session_id: Option<String> = None;
    let mut remote_path = None;
    let mut task_id = None;
    let mut temp_file = None;
    let mut authorized = false;
    while let Some(mut field) = multipart.next_field().await.map_err(bad_request)? {
        match field.name() {
            Some("sessionId") => session_id = Some(field.text().await.map_err(bad_request)?),
            Some("remotePath") => remote_path = Some(field.text().await.map_err(bad_request)?),
            Some("taskId") => task_id = Some(field.text().await.map_err(bad_request)?),
            Some("file") => {
                // Authorization must succeed before writing anything to disk (C-3 fix).
                let sid = session_id.as_deref().ok_or_else(|| bad_request("sessionId must precede the file field"))?;
                if !authorized {
                    authorize_session(&state, &headers, sid).await?;
                    authorized = true;
                }
                let path = state.data_dir.join(format!(".ssh-upload-{}", uuid::Uuid::new_v4()));
                let mut output = tokio::fs::File::create(&path).await.map_err(internal_error)?;
                let guard = TempFileGuard(path);
                while let Some(chunk) = field.chunk().await.map_err(bad_request)? {
                    output.write_all(&chunk).await.map_err(internal_error)?;
                }
                output.flush().await.map_err(internal_error)?;
                temp_file = Some(guard);
            }
            _ => {}
        }
    }
    let session_id = session_id.ok_or_else(|| bad_request("Missing sessionId"))?;
    // Ensure auth even if the file field was absent.
    if !authorized {
        authorize_session(&state, &headers, &session_id).await?;
    }
    let task_id = match task_id {
        Some(task_id) => uuid::Uuid::parse_str(&task_id).map_err(|_| bad_request("Invalid taskId"))?.to_string(),
        None => uuid::Uuid::new_v4().to_string(),
    };
    let remote_path = remote_path.ok_or_else(|| bad_request("Missing remotePath"))?;
    let temp_file = temp_file.ok_or_else(|| bad_request("Missing file"))?;
    let result = state.app.ssh_registry.sftp_upload_task(&session_id, &task_id, &temp_file.0, &remote_path).await;
    Ok(Json(result.map_err(internal_error)?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelTransferRequest {
    task_id: String,
}

pub async fn cancel_sftp_transfer(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<CancelTransferRequest>,
) -> Result<Json<()>, (StatusCode, String)> {
    let owner_session = request_owner(&state, &headers).await?;
    state
        .app
        .ssh_registry
        .cancel_sftp_transfer_owned(&request.task_id, &owner_session)
        .await
        .map_err(forbidden_error)?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDownloadRequest {
    session_id: String,
    path: String,
}

pub async fn create_sftp_download(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<CreateDownloadRequest>,
) -> Result<Json<SftpTransferTask>, (StatusCode, String)> {
    let owner_session = request_owner(&state, &headers).await?;
    state.app.ssh_registry.ensure_session_owner(&request.session_id, &owner_session).await.map_err(forbidden_error)?;
    let task_id = uuid::Uuid::new_v4().to_string();
    let stream = state
        .app
        .ssh_registry
        .sftp_download_stream_owned(&request.session_id, &owner_session, &task_id, &request.path)
        .await
        .map_err(internal_error)?;
    let task = stream.task.clone();
    state
        .ssh_downloads
        .write()
        .await
        .insert(task_id.clone(), SshWebDownload { owner_session: owner_session.clone(), stream });

    let cleanup_state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let expired = cleanup_state.ssh_downloads.write().await.remove(&task_id);
        if expired.is_some() {
            let _ = cleanup_state.app.ssh_registry.cancel_sftp_transfer_owned(&task_id, &owner_session).await;
        }
    });
    Ok(Json(task))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadParams {
    task_id: String,
}

pub async fn sftp_download(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Query(params): Query<DownloadParams>,
) -> Result<Response, (StatusCode, String)> {
    let owner_session = request_owner(&state, &headers).await?;
    let pending = {
        let mut downloads = state.ssh_downloads.write().await;
        if downloads.get(&params.task_id).is_none_or(|download| download.owner_session != owner_session) {
            return Err(forbidden_error("SFTP download task was not found"));
        }
        downloads.remove(&params.task_id).expect("download ownership was checked")
    };
    let task = pending.stream.task;
    let mut chunks = pending.stream.chunks;
    let body_stream = async_stream::stream! {
        while let Some(chunk) = chunks.recv().await {
            yield chunk.map_err(std::io::Error::other);
        }
    };
    let mut response = Response::new(Body::from_stream(body_stream));
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    if let Ok(value) = HeaderValue::from_str(&task.size.to_string()) {
        response.headers_mut().insert(header::CONTENT_LENGTH, value);
    }
    let disposition = format!("attachment; filename=\"{}\"", task.file_name.replace('"', ""));
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        response.headers_mut().insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

pub async fn sftp_transfer_events(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let owner_session = request_owner(&state, &headers).await?;
    let mut receiver = state.app.ssh_registry.subscribe_transfer_progress();
    let stream = async_stream::stream! {
        loop {
            match receiver.recv().await {
                Ok(task) if task.owner_session.as_deref() == Some(owner_session.as_str()) => {
                    if let Ok(data) = serde_json::to_string(&task) {
                        yield Ok(Event::default().event("transfer").data(data));
                    }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWsParams {
    session_id: String,
    #[serde(default)]
    after_sequence: u64,
}

pub async fn terminal_ws(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Query(params): Query<TerminalWsParams>,
    State(state): State<Arc<WebState>>,
) -> Response {
    if !same_origin_websocket(&headers) {
        return (StatusCode::FORBIDDEN, "SSH terminal WebSocket requires a same-origin request").into_response();
    }
    let Ok(owner_session) = request_owner(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if state.app.ssh_registry.ensure_session_owner(&params.session_id, &owner_session).await.is_err() {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.on_upgrade(move |socket| handle_terminal_socket(socket, state, params)).into_response()
}

fn same_origin_websocket(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Some(host) = headers.get(header::HOST).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    origin
        .parse::<reqwest::Url>()
        .ok()
        .and_then(|url| url.host_str().map(|origin_host| (origin_host.to_string(), url.port_or_known_default())))
        .is_some_and(|(origin_host, origin_port)| {
            let request_host = host.split(':').next().unwrap_or(host);
            let request_port = host.rsplit_once(':').and_then(|(_, port)| port.parse::<u16>().ok());
            origin_host.eq_ignore_ascii_case(request_host) && (request_port.is_none() || request_port == origin_port)
        })
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum TerminalCommand {
    Input { data: String },
    Resize { cols: u32, rows: u32 },
    DirectoryTracking { enabled: bool },
    Ping,
}

async fn handle_terminal_socket(socket: WebSocket, state: Arc<WebState>, params: TerminalWsParams) {
    let Ok((replay, mut output_rx)) = state.app.ssh_registry.subscribe(&params.session_id, params.after_sequence).await
    else {
        return;
    };
    let (mut sender, mut receiver) = socket.split();
    for frame in replay {
        if sender.send(frame_message(frame)).await.is_err() {
            return;
        }
    }
    let input_state = state.clone();
    let session_id = params.session_id.clone();
    let input = tokio::spawn(async move {
        while let Some(Ok(message)) = receiver.next().await {
            let result = match message {
                Message::Binary(data) => input_state.app.ssh_registry.write_terminal(&session_id, data.to_vec()).await,
                Message::Text(text) => match serde_json::from_str::<TerminalCommand>(&text) {
                    Ok(TerminalCommand::Input { data }) => {
                        input_state.app.ssh_registry.write_terminal(&session_id, data.into_bytes()).await
                    }
                    Ok(TerminalCommand::Resize { cols, rows }) => {
                        input_state.app.ssh_registry.resize_terminal(&session_id, cols, rows).await
                    }
                    Ok(TerminalCommand::DirectoryTracking { enabled }) => {
                        input_state.app.ssh_registry.set_directory_tracking(&session_id, enabled).await
                    }
                    Ok(TerminalCommand::Ping) => Ok(()),
                    Err(error) => Err(error.to_string()),
                },
                Message::Close(_) => break,
                _ => Ok(()),
            };
            if result.is_err() {
                break;
            }
        }
    });
    while let Ok(frame) = output_rx.recv().await {
        if sender.send(frame_message(frame)).await.is_err() {
            break;
        }
    }
    input.abort();
}

fn frame_message(frame: TerminalFrame) -> Message {
    let stream = match frame.stream {
        TerminalStream::Stdout => 0,
        TerminalStream::Stderr => 1,
        TerminalStream::State => 2,
    };
    let mut bytes = Vec::with_capacity(frame.data.len() + 9);
    bytes.extend_from_slice(&frame.sequence.to_be_bytes());
    bytes.push(stream);
    bytes.extend_from_slice(&frame.data);
    Message::Binary(bytes.into())
}

fn internal_error(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn bad_request(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, error.to_string())
}

fn forbidden_error(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::FORBIDDEN, error.to_string())
}

async fn request_owner(state: &WebState, headers: &HeaderMap) -> Result<String, (StatusCode, String)> {
    crate::auth::require_session_token(state, headers)
        .await
        .map_err(|status| (status, "A valid DBX web session is required".to_string()))
}

async fn authorize_session(
    state: &WebState,
    headers: &HeaderMap,
    session_id: &str,
) -> Result<String, (StatusCode, String)> {
    let owner_session = request_owner(state, headers).await?;
    state.app.ssh_registry.ensure_session_owner(session_id, &owner_session).await.map_err(forbidden_error)?;
    Ok(owner_session)
}

struct TempFileGuard(std::path::PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
