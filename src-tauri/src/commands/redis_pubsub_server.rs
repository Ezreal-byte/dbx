use std::sync::Arc;
use std::{net::Ipv4Addr, net::TcpListener};

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;

use dbx_core::connection::AppState;

const DEFAULT_PUBSUB_PORT: u16 = 4224;

pub struct PubSubServerPort(Option<u16>);

impl PubSubServerPort {
    fn new(port: Option<u16>) -> Self {
        Self(port)
    }

    fn get(&self) -> Result<u16, String> {
        self.0.ok_or_else(|| "Redis PubSub server is unavailable".to_string())
    }

    pub(crate) fn ssh_get(&self) -> Result<u16, String> {
        self.0.ok_or_else(|| "SSH terminal server is unavailable".to_string())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PubSubWsParams {
    connection_id: String,
}

pub fn build_pubsub_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/redis/pubsub/ws", get(ws_handler))
        .route("/api/ssh/terminal/ws", get(ssh_ws_handler))
        .with_state(state)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SshWsParams {
    session_id: String,
    #[serde(default)]
    after_sequence: u64,
}

async fn ssh_ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<SshWsParams>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ssh_socket(socket, state, params))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum SshTerminalCommand {
    Input { data: String },
    Resize { cols: u32, rows: u32 },
    DirectoryTracking { enabled: bool },
    Ping,
}

async fn handle_ssh_socket(socket: WebSocket, state: Arc<AppState>, params: SshWsParams) {
    let subscription = state.ssh_registry.subscribe(&params.session_id, params.after_sequence).await;
    let (replay, mut output_rx) = match subscription {
        Ok(value) => value,
        Err(error) => {
            let (mut sender, _) = socket.split();
            let payload = serde_json::json!({ "type": "error", "message": error }).to_string();
            let _ = sender.send(Message::Text(payload.into())).await;
            return;
        }
    };
    let (mut sender, mut receiver) = socket.split();
    for frame in replay {
        if sender.send(ssh_frame_message(frame)).await.is_err() {
            return;
        }
    }

    let registry = state.clone();
    let session_id = params.session_id.clone();
    let input = tokio::spawn(async move {
        while let Some(Ok(message)) = receiver.next().await {
            let result = match message {
                Message::Binary(data) => registry.ssh_registry.write_terminal(&session_id, data.to_vec()).await,
                Message::Text(text) => match serde_json::from_str::<SshTerminalCommand>(&text) {
                    Ok(SshTerminalCommand::Input { data }) => {
                        registry.ssh_registry.write_terminal(&session_id, data.into_bytes()).await
                    }
                    Ok(SshTerminalCommand::Resize { cols, rows }) => {
                        registry.ssh_registry.resize_terminal(&session_id, cols, rows).await
                    }
                    Ok(SshTerminalCommand::DirectoryTracking { enabled }) => {
                        registry.ssh_registry.set_directory_tracking(&session_id, enabled).await
                    }
                    Ok(SshTerminalCommand::Ping) => Ok(()),
                    Err(error) => Err(format!("Invalid SSH terminal command: {error}")),
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
        if sender.send(ssh_frame_message(frame)).await.is_err() {
            break;
        }
    }
    input.abort();
}

fn ssh_frame_message(frame: dbx_core::ssh_workbench::TerminalFrame) -> Message {
    let stream = match frame.stream {
        dbx_core::ssh_workbench::TerminalStream::Stdout => 0u8,
        dbx_core::ssh_workbench::TerminalStream::Stderr => 1u8,
        dbx_core::ssh_workbench::TerminalStream::State => 2u8,
    };
    let mut payload = Vec::with_capacity(9 + frame.data.len());
    payload.extend_from_slice(&frame.sequence.to_be_bytes());
    payload.push(stream);
    payload.extend_from_slice(&frame.data);
    Message::Binary(payload.into())
}

fn pubsub_server_port() -> u16 {
    std::env::var("DBX_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PUBSUB_PORT)
}

#[tauri::command]
pub fn redis_pubsub_server_port(port: tauri::State<'_, PubSubServerPort>) -> Result<u16, String> {
    port.get()
}

fn bind_pubsub_listener(preferred_port: u16) -> Result<TcpListener, String> {
    let preferred_addr = (Ipv4Addr::LOCALHOST, preferred_port);
    match TcpListener::bind(preferred_addr) {
        Ok(listener) => Ok(listener),
        Err(preferred_error) if preferred_port != 0 => {
            log::warn!(
                "Failed to bind PubSub server on {preferred_addr:?}: {preferred_error}; using an available port instead"
            );
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|fallback_error| {
                format!("Failed to bind PubSub server on an available port: {fallback_error}")
            })
        }
        Err(error) => Err(format!("Failed to bind PubSub server on {preferred_addr:?}: {error}")),
    }
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<PubSubWsParams>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let connection_id = params.connection_id;
    ws.on_upgrade(move |socket| handle_socket(socket, state, connection_id))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>, connection_id: String) {
    // Create PubSub connection
    let pubsub = match dbx_core::redis_ops::redis_create_pubsub_core(&state, &connection_id).await {
        Ok(p) => p,
        Err(e) => {
            let (mut sender, _) = socket.split();
            let _ = sender.send(Message::Text(format!(r#"{{"error":"{e}"}}"#).into())).await;
            return;
        }
    };

    let (mut sink, mut stream) = pubsub.split();
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Channel for WebSocket commands -> PubSub sink
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Task: Read WebSocket commands
    let ws_read = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                Message::Text(text) => {
                    if cmd_tx.send(text.to_string()).is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // Task: Apply commands to PubSub sink
    let sink_handle = tokio::spawn(async move {
        while let Some(text) = cmd_rx.recv().await {
            if let Err(e) = handle_command(&mut sink, &text).await {
                log::warn!("PubSub command error: {e}");
            }
        }
    });

    // Forward Redis messages to WebSocket (uses ws_sender, no mutex contention)
    while let Some(msg) = stream.next().await {
        let payload: String = msg.get_payload().unwrap_or_default();
        let channel = msg.get_channel_name().to_string();
        let pattern: Option<String> = msg.get_pattern().ok();
        let json = serde_json::json!({
            "channel": channel,
            "pattern": pattern,
            "payload": payload,
        });
        let text = serde_json::to_string(&json).unwrap_or_default();
        if ws_sender.send(Message::Text(text.into())).await.is_err() {
            break;
        }
    }

    ws_read.abort();
    sink_handle.abort();
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum PubSubCommand {
    #[serde(rename = "subscribe")]
    Subscribe { channels: Vec<String> },
    #[serde(rename = "psubscribe")]
    Psubscribe { patterns: Vec<String> },
    #[serde(rename = "unsubscribe")]
    Unsubscribe { channels: Vec<String> },
    #[serde(rename = "punsubscribe")]
    Punsubscribe { patterns: Vec<String> },
}

async fn handle_command(sink: &mut redis::aio::PubSubSink, text: &str) -> Result<(), String> {
    let cmd: PubSubCommand = serde_json::from_str(text).map_err(|e| format!("Invalid PubSub command: {e}"))?;

    match cmd {
        PubSubCommand::Subscribe { channels } => {
            for ch in &channels {
                sink.subscribe(ch).await.map_err(|e| format!("Subscribe error: {e}"))?;
            }
        }
        PubSubCommand::Psubscribe { patterns } => {
            for pat in &patterns {
                sink.psubscribe(pat).await.map_err(|e| format!("PSubscribe error: {e}"))?;
            }
        }
        PubSubCommand::Unsubscribe { channels } => {
            for ch in &channels {
                sink.unsubscribe(ch).await.map_err(|e| format!("Unsubscribe error: {e}"))?;
            }
        }
        PubSubCommand::Punsubscribe { patterns } => {
            for pat in &patterns {
                sink.punsubscribe(pat).await.map_err(|e| format!("PUnsubscribe error: {e}"))?;
            }
        }
    }
    Ok(())
}

/// Start the embedded web server for PubSub WebSocket support.
/// Runs on a background task using the shared AppState.
pub fn start_pubsub_server(state: Arc<AppState>) -> PubSubServerPort {
    let router = build_pubsub_router(state);
    let listener = match bind_pubsub_listener(pubsub_server_port()) {
        Ok(listener) => listener,
        Err(error) => {
            log::warn!("{error}");
            return PubSubServerPort::new(None);
        }
    };
    let addr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(error) => {
            log::warn!("Failed to read PubSub server address: {error}");
            return PubSubServerPort::new(None);
        }
    };
    if let Err(error) = listener.set_nonblocking(true) {
        log::warn!("Failed to configure PubSub server listener: {error}");
        return PubSubServerPort::new(None);
    }

    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(error) => {
                log::warn!("Failed to start PubSub server on {addr}: {error}");
                return;
            }
        };
        log::info!("PubSub WebSocket server listening on {addr}");
        if let Err(error) = axum::serve(listener, router).await {
            log::warn!("PubSub server stopped with error: {error}");
        }
    });

    PubSubServerPort::new(Some(addr.port()))
}

#[cfg(test)]
mod tests {
    use super::bind_pubsub_listener;
    use std::net::{Ipv4Addr, TcpListener};

    #[test]
    fn falls_back_to_an_available_local_port_when_the_preferred_port_is_in_use() {
        let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let preferred_port = occupied.local_addr().unwrap().port();

        let listener = bind_pubsub_listener(preferred_port).unwrap();

        assert_eq!(listener.local_addr().unwrap().ip(), Ipv4Addr::LOCALHOST);
        assert_ne!(listener.local_addr().unwrap().port(), preferred_port);
    }
}
