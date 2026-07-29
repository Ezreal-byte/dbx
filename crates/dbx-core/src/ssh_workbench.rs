use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use russh::client::Handle;
use russh::{ChannelMsg, Disconnect};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileType;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::db::ssh_tunnel::{connect_and_authenticate_with_keepalive, SshClient};
use crate::models::connection::ConnectionConfig;

const TERMINAL_BUFFER_LIMIT: usize = 2 * 1024 * 1024;
const SFTP_TRANSFER_CHUNK_SIZE: usize = 64 * 1024;
const SFTP_MAX_CONCURRENT_TRANSFERS: usize = 3;
const DIRECTORY_HANDSHAKE_LIMIT: usize = 64 * 1024;
const DIRECTORY_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
const DISCONNECTED_SESSION_TTL: Duration = Duration::from_secs(60);
const REMOTE_SHELL_DETECTION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SshWorkbenchConfig {
    #[serde(default = "default_auth_method")]
    pub auth_method: String,
    #[serde(default)]
    pub key_path: String,
    #[serde(default)]
    pub key_passphrase: String,
    #[serde(default)]
    pub use_ssh_agent: bool,
    #[serde(default)]
    pub ssh_agent_sock_path: String,
    #[serde(default = "default_terminal_type")]
    pub terminal_type: String,
    #[serde(default = "default_cols")]
    pub cols: u32,
    #[serde(default = "default_rows")]
    pub rows: u32,
}

fn default_auth_method() -> String {
    "password".to_string()
}

fn default_terminal_type() -> String {
    "xterm-256color".to_string()
}

fn default_cols() -> u32 {
    120
}

fn default_rows() -> u32 {
    32
}

impl Default for SshWorkbenchConfig {
    fn default() -> Self {
        Self {
            auth_method: default_auth_method(),
            key_path: String::new(),
            key_passphrase: String::new(),
            use_ssh_agent: false,
            ssh_agent_sock_path: String::new(),
            terminal_type: default_terminal_type(),
            cols: default_cols(),
            rows: default_rows(),
        }
    }
}

impl SshWorkbenchConfig {
    pub fn from_connection(config: &ConnectionConfig) -> Self {
        config
            .external_config
            .as_ref()
            .and_then(|value| value.get("sshWorkbench"))
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionInfo {
    pub session_id: String,
    pub connection_id: String,
    pub connected: bool,
    pub sequence: u64,
    pub directory_tracking_supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalFrame {
    pub session_id: String,
    pub sequence: u64,
    pub data: Vec<u8>,
    pub stream: TerminalStream,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalStream {
    Stdout,
    Stderr,
    State,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub kind: SftpEntryKind,
    pub size: Option<u64>,
    pub modified_at: Option<u64>,
    pub permissions: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SftpEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SftpTransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SftpTransferStatus {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferTask {
    pub task_id: String,
    pub session_id: String,
    pub direction: SftpTransferDirection,
    pub file_name: String,
    pub size: u64,
    pub transferred: u64,
    pub status: SftpTransferStatus,
    pub error: Option<String>,
    #[serde(skip)]
    pub owner_session: Option<String>,
}

pub struct SftpDownloadStream {
    pub task: SftpTransferTask,
    pub chunks: mpsc::Receiver<Result<Vec<u8>, String>>,
}

enum TerminalCommand {
    Input(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    DirectoryTracking { enabled: bool },
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteShell {
    Bash,
    Zsh,
    Other,
}

#[derive(Default)]
struct DirectoryHandshakeFilter {
    marker: Option<Vec<u8>>,
    buffered: Vec<u8>,
    started_at: Option<Instant>,
    failed: bool,
}

impl DirectoryHandshakeFilter {
    fn begin(&mut self, marker: Vec<u8>) {
        self.marker = Some(marker);
        self.buffered.clear();
        self.started_at = Some(Instant::now());
        self.failed = false;
    }

    fn filter(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        let Some(marker) = self.marker.as_ref() else {
            return Some(data.to_vec());
        };
        self.buffered.extend_from_slice(data);
        if self.buffered.len() > DIRECTORY_HANDSHAKE_LIMIT {
            return self.fail_open();
        }
        let Some(index) = find_bytes(&self.buffered, marker) else {
            return None;
        };
        let result = self.buffered[index + marker.len()..].to_vec();
        self.marker = None;
        self.buffered.clear();
        self.started_at = None;
        (!result.is_empty()).then_some(result)
    }

    fn flush_if_timed_out(&mut self) -> Option<Vec<u8>> {
        if self.started_at.is_some_and(|started_at| started_at.elapsed() >= DIRECTORY_HANDSHAKE_TIMEOUT) {
            return self.fail_open();
        }
        None
    }

    fn take_failed(&mut self) -> bool {
        std::mem::take(&mut self.failed)
    }

    fn fail_open(&mut self) -> Option<Vec<u8>> {
        self.marker = None;
        self.started_at = None;
        self.failed = true;
        let buffered = std::mem::take(&mut self.buffered);
        (!buffered.is_empty()).then_some(buffered)
    }
}

struct BufferedFrame {
    frame: TerminalFrame,
    bytes: usize,
}

struct SessionEntry {
    connection_id: String,
    owner_session: Option<String>,
    read_only: bool,
    connected: AtomicBool,
    handle: Arc<Handle<SshClient>>,
    terminal_tx: mpsc::Sender<TerminalCommand>,
    output_tx: broadcast::Sender<TerminalFrame>,
    replay: Arc<Mutex<VecDeque<BufferedFrame>>>,
    sftp: Mutex<Option<Arc<Mutex<SftpSession>>>>,
    transfer_limit: Arc<Semaphore>,
}

struct TransferRegistration {
    session_id: String,
    owner_session: Option<String>,
    cancellation: CancellationToken,
}

struct TransferCleanupGuard {
    task_id: String,
    task: StdMutex<SftpTransferTask>,
    registrations: Arc<StdMutex<HashMap<String, TransferRegistration>>>,
    transfer_tx: broadcast::Sender<SftpTransferTask>,
    cancellation: CancellationToken,
    artifact_cleanup_tx: mpsc::UnboundedSender<TransferArtifactCleanup>,
    artifact_cleanup: Option<TransferArtifactCleanup>,
    armed: bool,
}

enum TransferArtifactCleanup {
    Remote { sftp: Arc<Mutex<SftpSession>>, temporary_path: String, target_path: String, backup_path: String },
    Local { temporary_path: PathBuf, target_path: PathBuf, backup_path: PathBuf },
}

impl TransferCleanupGuard {
    fn new(
        task: SftpTransferTask,
        registrations: Arc<StdMutex<HashMap<String, TransferRegistration>>>,
        transfer_tx: broadcast::Sender<SftpTransferTask>,
        cancellation: CancellationToken,
    ) -> Self {
        let (artifact_cleanup_tx, mut artifact_cleanup_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            if let Some(cleanup) = artifact_cleanup_rx.recv().await {
                cleanup_transfer_artifact(cleanup).await;
            }
        });
        Self {
            task_id: task.task_id.clone(),
            task: StdMutex::new(task),
            registrations,
            transfer_tx,
            cancellation,
            artifact_cleanup_tx,
            artifact_cleanup: None,
            armed: true,
        }
    }

    fn update(&self, task: &SftpTransferTask) {
        if let Ok(mut current) = self.task.lock() {
            *current = task.clone();
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn set_artifact_cleanup(&mut self, cleanup: TransferArtifactCleanup) {
        self.artifact_cleanup = Some(cleanup);
    }
}

impl Drop for TransferCleanupGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.cancellation.cancel();
        if let Some(cleanup) = self.artifact_cleanup.take() {
            let _ = self.artifact_cleanup_tx.send(cleanup);
        }
        let task_id = self.task_id.clone();
        let registrations = self.registrations.clone();
        let transfer_tx = self.transfer_tx.clone();
        let mut task = self.task.lock().map(|task| task.clone()).unwrap_or_else(|_| SftpTransferTask {
            task_id: task_id.clone(),
            session_id: String::new(),
            direction: SftpTransferDirection::Upload,
            file_name: String::new(),
            size: 0,
            transferred: 0,
            status: SftpTransferStatus::Failed,
            error: Some("SFTP transfer interrupted before completion".to_string()),
            owner_session: None,
        });
        task.status = SftpTransferStatus::Failed;
        task.error = Some("SFTP transfer interrupted before completion".to_string());
        if let Ok(mut registrations) = registrations.lock() {
            registrations.remove(&task_id);
        }
        let _ = transfer_tx.send(task);
    }
}

pub struct SshSessionRegistry {
    sessions: Arc<RwLock<HashMap<String, Arc<SessionEntry>>>>,
    known_hosts_path: PathBuf,
    transfer_tx: broadcast::Sender<SftpTransferTask>,
    transfer_cancellations: Arc<StdMutex<HashMap<String, TransferRegistration>>>,
}

impl SshSessionRegistry {
    pub fn new(data_dir: PathBuf) -> Self {
        let (transfer_tx, _) = broadcast::channel(256);
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            known_hosts_path: data_dir.join("known_hosts"),
            transfer_tx,
            transfer_cancellations: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    pub fn subscribe_transfer_progress(&self) -> broadcast::Receiver<SftpTransferTask> {
        self.transfer_tx.subscribe()
    }

    pub async fn cancel_sftp_transfer(&self, task_id: &str) -> Result<(), String> {
        let cancellation = self
            .transfer_cancellations
            .lock()
            .map_err(|_| "SFTP transfer registry is unavailable".to_string())?
            .get(task_id)
            .map(|registration| registration.cancellation.clone())
            .ok_or_else(|| "SFTP transfer task was not found".to_string())?;
        cancellation.cancel();
        Ok(())
    }

    pub async fn cancel_sftp_transfer_owned(&self, task_id: &str, owner_session: &str) -> Result<(), String> {
        let cancellation = self
            .transfer_cancellations
            .lock()
            .map_err(|_| "SFTP transfer registry is unavailable".to_string())?
            .get(task_id)
            .filter(|registration| registration.owner_session.as_deref() == Some(owner_session))
            .map(|registration| registration.cancellation.clone())
            .ok_or_else(|| "SFTP transfer task was not found".to_string())?;
        cancellation.cancel();
        Ok(())
    }

    pub async fn test_connection(&self, config: &ConnectionConfig) -> Result<(), String> {
        let workbench = SshWorkbenchConfig::from_connection(config);
        let handle = connect_config(config, &workbench, &self.known_hosts_path).await?;
        handle
            .disconnect(Disconnect::ByApplication, "DBX SSH connection test complete", "English")
            .await
            .map_err(|error| format!("SSH disconnect failed: {error}"))
    }

    pub async fn create_session(
        &self,
        config: &ConnectionConfig,
        cols: u32,
        rows: u32,
    ) -> Result<SshSessionInfo, String> {
        self.create_session_owned(config, cols, rows, None).await
    }

    pub async fn create_session_owned(
        &self,
        config: &ConnectionConfig,
        cols: u32,
        rows: u32,
        owner_session: Option<String>,
    ) -> Result<SshSessionInfo, String> {
        let workbench = SshWorkbenchConfig::from_connection(config);
        let handle = Arc::new(connect_config(config, &workbench, &self.known_hosts_path).await?);
        let remote_shell = detect_remote_shell(&handle).await;
        let directory_tracking_supported = remote_shell.supports_directory_tracking();
        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(|error| format!("Failed to open SSH terminal channel: {error}"))?;
        channel
            .request_pty(true, &workbench.terminal_type, cols.max(1), rows.max(1), 0, 0, &[])
            .await
            .map_err(|error| format!("Failed to request SSH PTY: {error}"))?;
        channel.request_shell(true).await.map_err(|error| format!("Failed to start SSH shell: {error}"))?;

        let session_id = Uuid::new_v4().to_string();
        let (terminal_tx, mut terminal_rx) = mpsc::channel::<TerminalCommand>(256);
        let (output_tx, _) = broadcast::channel::<TerminalFrame>(1024);
        let replay = Arc::new(Mutex::new(VecDeque::new()));
        let replay_bytes = Arc::new(Mutex::new(0usize));
        let sequence = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let entry = Arc::new(SessionEntry {
            connection_id: config.id.clone(),
            owner_session,
            read_only: config.read_only,
            connected: AtomicBool::new(true),
            handle: handle.clone(),
            terminal_tx,
            output_tx: output_tx.clone(),
            replay: replay.clone(),
            sftp: Mutex::new(None),
            transfer_limit: Arc::new(Semaphore::new(SFTP_MAX_CONCURRENT_TRANSFERS)),
        });

        let task_session_id = session_id.clone();
        let directory_marker_id = session_id.clone();
        let task_entry = entry.clone();
        let sessions = self.sessions.clone();
        let transfer_cancellations = self.transfer_cancellations.clone();
        tokio::spawn(async move {
            let mut directory_filter = DirectoryHandshakeFilter::default();
            let mut directory_tracking_enabled = false;
            let mut intentional_close = false;
            let mut disconnect_stage = "remote-channel";
            let mut directory_timeout = tokio::time::interval(Duration::from_millis(250));
            directory_timeout.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = directory_timeout.tick() => {
                        if let Some(data) = directory_filter.flush_if_timed_out() {
                            publish_frame(
                                &task_session_id,
                                data,
                                TerminalStream::Stdout,
                                &output_tx,
                                &replay,
                                &replay_bytes,
                                &sequence,
                            ).await;
                        }
                        if directory_filter.take_failed() {
                            directory_tracking_enabled = false;
                            publish_frame(
                                &task_session_id,
                                b"directory-tracking-unavailable".to_vec(),
                                TerminalStream::State,
                                &output_tx,
                                &replay,
                                &replay_bytes,
                                &sequence,
                            ).await;
                        }
                    }
                    command = terminal_rx.recv() => {
                        match command {
                            Some(TerminalCommand::Input(data)) => {
                                if channel.data(&data[..]).await.is_err() {
                                    disconnect_stage = "terminal-write";
                                    break;
                                }
                            }
                            Some(TerminalCommand::Resize { cols, rows }) => {
                                let _ = channel.window_change(cols.max(1), rows.max(1), 0, 0).await;
                            }
                            Some(TerminalCommand::DirectoryTracking { enabled }) => {
                                if remote_shell == RemoteShell::Other || directory_tracking_enabled == enabled {
                                    continue;
                                }
                                directory_tracking_enabled = enabled;
                                let marker = directory_tracking_marker(&directory_marker_id);
                                directory_filter.begin(marker.clone());
                                publish_frame(
                                    &task_session_id,
                                    b"\r\x1b[2K".to_vec(),
                                    TerminalStream::Stdout,
                                    &output_tx,
                                    &replay,
                                    &replay_bytes,
                                    &sequence,
                                ).await;
                                let script = directory_tracking_script(enabled, &directory_marker_id, remote_shell);
                                if channel.data(script.as_bytes()).await.is_err() {
                                    disconnect_stage = "directory-tracking-write";
                                    break;
                                }
                            }
                            Some(TerminalCommand::Close) | None => {
                                intentional_close = true;
                                let _ = channel.close().await;
                                break;
                            }
                        }
                    }
                    message = channel.wait() => {
                        let (data, stream) = match message {
                            Some(ChannelMsg::Data { data }) => (data.to_vec(), TerminalStream::Stdout),
                            Some(ChannelMsg::ExtendedData { data, .. }) => (data.to_vec(), TerminalStream::Stderr),
                            Some(ChannelMsg::Eof | ChannelMsg::Close) | None => break,
                            _ => continue,
                        };
                        let Some(data) = directory_filter.filter(&data) else {
                            continue;
                        };
                        publish_frame(
                            &task_session_id,
                            data,
                            stream,
                            &output_tx,
                            &replay,
                            &replay_bytes,
                            &sequence,
                        ).await;
                        if directory_filter.take_failed() {
                            directory_tracking_enabled = false;
                            publish_frame(
                                &task_session_id,
                                b"directory-tracking-unavailable".to_vec(),
                                TerminalStream::State,
                                &output_tx,
                                &replay,
                                &replay_bytes,
                                &sequence,
                            ).await;
                        }
                    }
                }
            }
            task_entry.connected.store(false, Ordering::Release);
            if intentional_close {
                log::debug!(
                    "SSH workbench session closed by application: connection_id={}, session_id={}",
                    task_entry.connection_id,
                    task_session_id
                );
            } else {
                log::warn!(
                    "SSH workbench transport disconnected: connection_id={}, session_id={}, stage={disconnect_stage}",
                    task_entry.connection_id,
                    task_session_id
                );
                publish_frame(
                    &task_session_id,
                    b"ssh-transport-disconnected".to_vec(),
                    TerminalStream::State,
                    &output_tx,
                    &replay,
                    &replay_bytes,
                    &sequence,
                )
                .await;
            }
            cancel_session_transfers(&transfer_cancellations, &task_session_id);
            tokio::time::sleep(DISCONNECTED_SESSION_TTL).await;
            let mut sessions = sessions.write().await;
            if sessions
                .get(&task_session_id)
                .is_some_and(|current| Arc::ptr_eq(current, &task_entry) && !current.connected.load(Ordering::Acquire))
            {
                sessions.remove(&task_session_id);
            }
        });

        self.sessions.write().await.insert(session_id.clone(), entry);
        Ok(SshSessionInfo {
            session_id,
            connection_id: config.id.clone(),
            connected: true,
            sequence: 0,
            directory_tracking_supported,
        })
    }

    pub async fn ensure_session_owner(&self, session_id: &str, owner_session: &str) -> Result<(), String> {
        let entry = self.session(session_id).await?;
        if entry.owner_session.as_deref() != Some(owner_session) {
            return Err("SSH session was not found or is not owned by this web session".to_string());
        }
        Ok(())
    }

    pub async fn close_sessions_by_owner(&self, owner_session: &str) {
        let ids: Vec<String> = self
            .sessions
            .read()
            .await
            .iter()
            .filter(|(_, entry)| entry.owner_session.as_deref() == Some(owner_session))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let _ = self.close_session(&id).await;
        }
    }

    pub async fn subscribe(
        &self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<(Vec<TerminalFrame>, broadcast::Receiver<TerminalFrame>), String> {
        let entry = self.session(session_id).await?;
        let replay = entry
            .replay
            .lock()
            .await
            .iter()
            .filter(|buffered| buffered.frame.sequence > after_sequence)
            .map(|buffered| buffered.frame.clone())
            .collect();
        Ok((replay, entry.output_tx.subscribe()))
    }

    pub async fn write_terminal(&self, session_id: &str, data: Vec<u8>) -> Result<(), String> {
        self.session(session_id)
            .await?
            .terminal_tx
            .send(TerminalCommand::Input(data))
            .await
            .map_err(|_| "SSH terminal is disconnected".to_string())
    }

    pub async fn resize_terminal(&self, session_id: &str, cols: u32, rows: u32) -> Result<(), String> {
        self.session(session_id)
            .await?
            .terminal_tx
            .send(TerminalCommand::Resize { cols, rows })
            .await
            .map_err(|_| "SSH terminal is disconnected".to_string())
    }

    pub async fn set_directory_tracking(&self, session_id: &str, enabled: bool) -> Result<(), String> {
        self.session(session_id)
            .await?
            .terminal_tx
            .send(TerminalCommand::DirectoryTracking { enabled })
            .await
            .map_err(|_| "SSH terminal session is closed".to_string())
    }

    pub async fn close_session(&self, session_id: &str) -> Result<(), String> {
        let Some(entry) = self.sessions.write().await.remove(session_id) else {
            return Ok(());
        };
        entry.connected.store(false, Ordering::Release);
        self.cancel_session_transfers(session_id).await;
        let _ = entry.terminal_tx.send(TerminalCommand::Close).await;
        entry
            .handle
            .disconnect(Disconnect::ByApplication, "DBX SSH workbench closed", "English")
            .await
            .map_err(|error| format!("SSH disconnect failed: {error}"))
    }

    pub async fn close_connection_sessions(&self, connection_id: &str) {
        let ids: Vec<String> = self
            .sessions
            .read()
            .await
            .iter()
            .filter(|(_, entry)| entry.connection_id == connection_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let _ = self.close_session(&id).await;
        }
    }

    pub async fn close_all_sessions(&self) {
        let ids: Vec<String> = self.sessions.read().await.keys().cloned().collect();
        for id in ids {
            let _ = self.close_session(&id).await;
        }
    }

    pub async fn sftp_home(&self, session_id: &str) -> Result<String, String> {
        let sftp = self.sftp_session(session_id).await?;
        let result = sftp.lock().await.canonicalize(".").await.map_err(sftp_error)?;
        Ok(result)
    }

    pub async fn sftp_list(&self, session_id: &str, path: &str) -> Result<Vec<SftpEntry>, String> {
        let sftp = self.sftp_session(session_id).await?;
        let path = normalize_remote_path(&sftp, path).await?;
        let entries = sftp.lock().await.read_dir(path).await.map_err(sftp_error)?;
        let mut result: Vec<SftpEntry> = entries
            .map(|entry| {
                let metadata = entry.metadata();
                let kind = match entry.file_type() {
                    FileType::File => SftpEntryKind::File,
                    FileType::Dir => SftpEntryKind::Directory,
                    FileType::Symlink => SftpEntryKind::Symlink,
                    FileType::Other => SftpEntryKind::Other,
                };
                SftpEntry {
                    name: entry.file_name(),
                    path: entry.path(),
                    kind,
                    size: metadata.size,
                    modified_at: metadata.mtime.map(u64::from),
                    permissions: metadata.permissions.map(format_permissions),
                }
            })
            .collect();
        result.sort_by(|left, right| {
            let left_dir = left.kind == SftpEntryKind::Directory;
            let right_dir = right.kind == SftpEntryKind::Directory;
            right_dir.cmp(&left_dir).then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok(result)
    }

    pub async fn sftp_mkdir(&self, session_id: &str, path: &str) -> Result<(), String> {
        self.ensure_writable(session_id).await?;
        let sftp = self.sftp_session(session_id).await?;
        let path = normalize_remote_path(&sftp, path).await?;
        let result = sftp.lock().await.create_dir(path).await.map_err(sftp_error);
        result
    }

    pub async fn sftp_rename(&self, session_id: &str, from: &str, to: &str) -> Result<(), String> {
        self.ensure_writable(session_id).await?;
        let sftp = self.sftp_session(session_id).await?;
        let from = normalize_remote_path(&sftp, from).await?;
        let to = normalize_remote_path(&sftp, to).await?;
        let result = sftp.lock().await.rename(from, to).await.map_err(sftp_error);
        result
    }

    pub async fn sftp_delete(&self, session_id: &str, path: &str, recursive: bool) -> Result<(), String> {
        self.ensure_writable(session_id).await?;
        let sftp = self.sftp_session(session_id).await?;
        let path = normalize_remote_path(&sftp, path).await?;
        delete_remote(sftp, path, recursive).await
    }

    pub async fn sftp_read(&self, session_id: &str, path: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
        let sftp = self.sftp_session(session_id).await?;
        let path = normalize_remote_path(&sftp, path).await?;
        let file = sftp.lock().await.open(path).await.map_err(sftp_error)?;
        let mut bytes = Vec::new();
        file.take(max_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| format!("SFTP read failed: {error}"))?;
        if bytes.len() > max_bytes {
            return Err(format!("Remote file exceeds the {max_bytes} byte preview limit"));
        }
        Ok(bytes)
    }

    pub async fn sftp_upload(&self, session_id: &str, local_path: &Path, remote_path: &str) -> Result<u64, String> {
        let task_id = Uuid::new_v4().to_string();
        self.sftp_upload_task(session_id, &task_id, local_path, remote_path).await.map(|task| task.transferred)
    }

    pub async fn sftp_upload_task(
        &self,
        session_id: &str,
        task_id: &str,
        local_path: &Path,
        remote_path: &str,
    ) -> Result<SftpTransferTask, String> {
        let entry = self.session(session_id).await?;
        self.ensure_writable(session_id).await?;
        let size = tokio::fs::metadata(local_path)
            .await
            .map_err(|error| format!("Failed to inspect local file: {error}"))?
            .len();
        let file_name =
            remote_path.rsplit('/').next().filter(|value| !value.is_empty()).unwrap_or("upload").to_string();
        let mut task = SftpTransferTask {
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            direction: SftpTransferDirection::Upload,
            file_name,
            size,
            transferred: 0,
            status: SftpTransferStatus::Queued,
            error: None,
            owner_session: entry.owner_session.clone(),
        };
        let cancellation = CancellationToken::new();
        self.register_transfer(task_id, session_id, &entry, cancellation.clone()).await?;
        let mut cleanup_guard = TransferCleanupGuard::new(
            task.clone(),
            self.transfer_cancellations.clone(),
            self.transfer_tx.clone(),
            cancellation.clone(),
        );
        let _ = self.transfer_tx.send(task.clone());
        let result: Result<u64, String> = async {
            let _permit = tokio::select! {
                permit = entry.transfer_limit.acquire() => {
                    permit.map_err(|_| "SSH session transfer queue is closed".to_string())?
                }
                _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
            };
            task.status = SftpTransferStatus::Running;
            cleanup_guard.update(&task);
            let _ = self.transfer_tx.send(task.clone());
            let sftp = tokio::select! {
                result = self.sftp_session(session_id) => result?,
                _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
            };
            let remote_path = normalize_remote_path(&sftp, remote_path).await?;
            let (temporary_path, backup_path) = remote_transfer_paths(&remote_path, task_id)?;
            cleanup_guard.set_artifact_cleanup(TransferArtifactCleanup::Remote {
                sftp: sftp.clone(),
                temporary_path: temporary_path.clone(),
                target_path: remote_path.clone(),
                backup_path: backup_path.clone(),
            });
            let mut source = tokio::select! {
                result = tokio::fs::File::open(local_path) => {
                    result.map_err(|error| format!("Failed to open local file: {error}"))?
                }
                _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
            };
            let mut target = tokio::select! {
                result = async { sftp.lock().await.create(temporary_path.clone()).await } => result.map_err(sftp_error)?,
                _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
            };
            let copy_result = copy_sftp_chunks(&mut source, &mut target, "upload", &cancellation, |transferred| {
                task.transferred = transferred;
                cleanup_guard.update(&task);
                let _ = self.transfer_tx.send(task.clone());
            })
            .await;
            drop(target);
            if let Err(error) = copy_result {
                remove_remote_file_if_present(&sftp, &temporary_path).await;
                return Err(error);
            }
            if cancellation.is_cancelled() {
                remove_remote_file_if_present(&sftp, &temporary_path).await;
                return Err("SFTP transfer cancelled".to_string());
            }
            commit_remote_file(&sftp, &temporary_path, &remote_path, &backup_path).await?;
            Ok(task.transferred)
        }
        .await;
        if let Ok(mut registrations) = self.transfer_cancellations.lock() {
            registrations.remove(task_id);
        }
        match result {
            Ok(transferred) => {
                task.transferred = transferred;
                task.status = SftpTransferStatus::Completed;
            }
            Err(_error) if cancellation.is_cancelled() => {
                task.status = SftpTransferStatus::Cancelled;
                task.error = None;
            }
            Err(error) => {
                task.status = SftpTransferStatus::Failed;
                task.error = Some(error);
            }
        }
        cleanup_guard.update(&task);
        let _ = self.transfer_tx.send(task.clone());
        cleanup_guard.disarm();
        Ok(task)
    }

    pub async fn sftp_download(&self, session_id: &str, remote_path: &str, local_path: &Path) -> Result<u64, String> {
        let task_id = Uuid::new_v4().to_string();
        self.sftp_download_task(session_id, &task_id, remote_path, local_path).await.map(|task| task.transferred)
    }

    pub async fn sftp_download_task(
        &self,
        session_id: &str,
        task_id: &str,
        remote_path: &str,
        local_path: &Path,
    ) -> Result<SftpTransferTask, String> {
        let entry = self.session(session_id).await?;
        let sftp = self.sftp_session(session_id).await?;
        let remote_path = normalize_remote_path(&sftp, remote_path).await?;
        let size = sftp.lock().await.metadata(remote_path.clone()).await.map_err(sftp_error)?.size.unwrap_or(0);
        let file_name = remote_path.rsplit('/').next().filter(|value| !value.is_empty()).unwrap_or("download");
        let (temporary_path, backup_path) = local_transfer_paths(local_path, task_id)?;
        let mut task = SftpTransferTask {
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            direction: SftpTransferDirection::Download,
            file_name: file_name.to_string(),
            size,
            transferred: 0,
            status: SftpTransferStatus::Queued,
            error: None,
            owner_session: entry.owner_session.clone(),
        };
        let cancellation = CancellationToken::new();
        self.register_transfer(task_id, session_id, &entry, cancellation.clone()).await?;
        let mut cleanup_guard = TransferCleanupGuard::new(
            task.clone(),
            self.transfer_cancellations.clone(),
            self.transfer_tx.clone(),
            cancellation.clone(),
        );
        cleanup_guard.set_artifact_cleanup(TransferArtifactCleanup::Local {
            temporary_path: temporary_path.clone(),
            target_path: local_path.to_path_buf(),
            backup_path: backup_path.clone(),
        });
        let _ = self.transfer_tx.send(task.clone());
        let result: Result<u64, String> = async {
            let _permit = tokio::select! {
                permit = entry.transfer_limit.acquire() => {
                    permit.map_err(|_| "SSH session transfer queue is closed".to_string())?
                }
                _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
            };
            task.status = SftpTransferStatus::Running;
            cleanup_guard.update(&task);
            let _ = self.transfer_tx.send(task.clone());
            let mut source = tokio::select! {
                result = async { sftp.lock().await.open(remote_path.clone()).await } => result.map_err(sftp_error)?,
                _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
            };
            let mut target = tokio::select! {
                result = tokio::fs::File::create(&temporary_path) => {
                    result.map_err(|error| format!("Failed to create local file: {error}"))?
                }
                _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
            };
            let copy_result = copy_sftp_chunks(&mut source, &mut target, "download", &cancellation, |transferred| {
                task.transferred = transferred;
                cleanup_guard.update(&task);
                let _ = self.transfer_tx.send(task.clone());
            })
            .await;
            drop(target);
            if let Err(error) = copy_result {
                remove_local_file_if_present(&temporary_path).await;
                return Err(error);
            }
            if cancellation.is_cancelled() {
                remove_local_file_if_present(&temporary_path).await;
                return Err("SFTP transfer cancelled".to_string());
            }
            commit_local_file(&temporary_path, local_path, &backup_path).await?;
            Ok(task.transferred)
        }
        .await;
        if let Ok(mut registrations) = self.transfer_cancellations.lock() {
            registrations.remove(task_id);
        }
        match result {
            Ok(transferred) => {
                task.transferred = transferred;
                task.status = SftpTransferStatus::Completed;
            }
            Err(_error) if cancellation.is_cancelled() => {
                task.status = SftpTransferStatus::Cancelled;
                task.error = None;
            }
            Err(error) => {
                task.status = SftpTransferStatus::Failed;
                task.error = Some(error);
            }
        }
        cleanup_guard.update(&task);
        let _ = self.transfer_tx.send(task.clone());
        cleanup_guard.disarm();
        Ok(task)
    }

    pub async fn sftp_download_stream_owned(
        &self,
        session_id: &str,
        owner_session: &str,
        task_id: &str,
        remote_path: &str,
    ) -> Result<SftpDownloadStream, String> {
        self.ensure_session_owner(session_id, owner_session).await?;
        let entry = self.session(session_id).await?;
        let sftp = self.sftp_session(session_id).await?;
        let remote_path = normalize_remote_path(&sftp, remote_path).await?;
        let size = sftp.lock().await.metadata(remote_path.clone()).await.map_err(sftp_error)?.size.unwrap_or(0);
        let file_name =
            remote_path.rsplit('/').next().filter(|value| !value.is_empty()).unwrap_or("download").to_string();
        let task = SftpTransferTask {
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            direction: SftpTransferDirection::Download,
            file_name,
            size,
            transferred: 0,
            status: SftpTransferStatus::Queued,
            error: None,
            owner_session: entry.owner_session.clone(),
        };
        let cancellation = CancellationToken::new();
        self.register_transfer(task_id, session_id, &entry, cancellation.clone()).await?;
        let _ = self.transfer_tx.send(task.clone());

        let (chunks_tx, chunks) = mpsc::channel::<Result<Vec<u8>, String>>(8);
        let task_template = task.clone();
        let transfer_tx = self.transfer_tx.clone();
        let registrations = self.transfer_cancellations.clone();
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            let mut current = task_template;
            let result: Result<u64, String> = async {
                let _permit = tokio::select! {
                    permit = entry.transfer_limit.clone().acquire_owned() => {
                        permit.map_err(|_| "SSH session transfer queue is closed".to_string())?
                    }
                    _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
                };
                current.status = SftpTransferStatus::Running;
                let _ = transfer_tx.send(current.clone());
                let mut source = tokio::select! {
                    result = async { sftp.lock().await.open(remote_path).await } => result.map_err(sftp_error)?,
                    _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
                };
                let mut transferred = 0_u64;
                let mut buffer = vec![0_u8; SFTP_TRANSFER_CHUNK_SIZE];
                loop {
                    let read = tokio::select! {
                        result = source.read(&mut buffer) => {
                            result.map_err(|error| format!("SFTP download failed: {error}"))?
                        }
                        _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
                    };
                    if read == 0 {
                        break;
                    }
                    let chunk = buffer[..read].to_vec();
                    tokio::select! {
                        result = chunks_tx.send(Ok(chunk)) => {
                            if result.is_err() {
                                return Err("SFTP download client disconnected".to_string());
                            }
                        }
                        _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
                    }
                    transferred = transferred.saturating_add(read as u64);
                    current.transferred = transferred;
                    let _ = transfer_tx.send(current.clone());
                }
                Ok(transferred)
            }
            .await;

            if let Ok(mut registrations) = registrations.lock() {
                registrations.remove(&task_id);
            }
            match result {
                Ok(transferred) => {
                    current.transferred = transferred;
                    current.status = SftpTransferStatus::Completed;
                }
                Err(_) if cancellation.is_cancelled() => {
                    current.status = SftpTransferStatus::Cancelled;
                    current.error = None;
                }
                Err(error) => {
                    current.status = SftpTransferStatus::Failed;
                    current.error = Some(error.clone());
                    let _ = chunks_tx.send(Err(error)).await;
                }
            }
            let _ = transfer_tx.send(current);
        });

        Ok(SftpDownloadStream { task, chunks })
    }

    async fn session(&self, session_id: &str) -> Result<Arc<SessionEntry>, String> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| "SSH session was not found or has expired".to_string())
    }

    async fn ensure_writable(&self, session_id: &str) -> Result<(), String> {
        ensure_sftp_writable(self.session(session_id).await?.read_only)
    }

    async fn register_transfer(
        &self,
        task_id: &str,
        session_id: &str,
        entry: &SessionEntry,
        cancellation: CancellationToken,
    ) -> Result<(), String> {
        self.transfer_cancellations.lock().map_err(|_| "SFTP transfer registry is unavailable".to_string())?.insert(
            task_id.to_string(),
            TransferRegistration {
                session_id: session_id.to_string(),
                owner_session: entry.owner_session.clone(),
                cancellation,
            },
        );
        Ok(())
    }

    async fn cancel_session_transfers(&self, session_id: &str) {
        cancel_session_transfers(&self.transfer_cancellations, session_id);
    }

    async fn sftp_session(&self, session_id: &str) -> Result<Arc<Mutex<SftpSession>>, String> {
        let entry = self.session(session_id).await?;
        if let Some(sftp) = entry.sftp.lock().await.as_ref().cloned() {
            return Ok(sftp);
        }
        let channel = entry
            .handle
            .channel_open_session()
            .await
            .map_err(|error| format!("Failed to open SFTP channel: {error}"))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| format!("Failed to start SFTP subsystem: {error}"))?;
        let session = Arc::new(Mutex::new(SftpSession::new(channel.into_stream()).await.map_err(sftp_error)?));
        *entry.sftp.lock().await = Some(session.clone());
        Ok(session)
    }
}

async fn normalize_remote_path(sftp: &Arc<Mutex<SftpSession>>, path: &str) -> Result<String, String> {
    if path.is_empty() {
        return Err("SFTP path cannot be empty".to_string());
    }
    if path.contains('\0') {
        return Err("SFTP path contains an invalid NUL character".to_string());
    }
    let absolute = if path.starts_with('/') {
        path.to_string()
    } else {
        let home = sftp.lock().await.canonicalize(".").await.map_err(sftp_error)?;
        format!("{}/{}", home.trim_end_matches('/'), path)
    };
    normalize_absolute_remote_path(&absolute)
}

fn normalize_absolute_remote_path(path: &str) -> Result<String, String> {
    if !path.starts_with('/') {
        return Err("SFTP path must resolve to an absolute remote path".to_string());
    }
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            value => components.push(value),
        }
    }
    Ok(if components.is_empty() { "/".to_string() } else { format!("/{}", components.join("/")) })
}

fn remote_transfer_paths(target_path: &str, task_id: &str) -> Result<(String, String), String> {
    let (parent, file_name) = target_path.rsplit_once('/').ok_or_else(|| "SFTP target path is invalid".to_string())?;
    if file_name.is_empty() {
        return Err("SFTP upload target must be a file path".to_string());
    }
    let parent = if parent.is_empty() { "/" } else { parent };
    Ok((
        format!("{}/.dbx-upload-{task_id}.part", parent.trim_end_matches('/')),
        format!("{}/.dbx-upload-{task_id}.backup", parent.trim_end_matches('/')),
    ))
}

async fn remove_remote_file_if_present(sftp: &Arc<Mutex<SftpSession>>, path: &str) {
    let _ = sftp.lock().await.remove_file(path.to_string()).await;
}

async fn cleanup_transfer_artifact(cleanup: TransferArtifactCleanup) {
    match cleanup {
        TransferArtifactCleanup::Remote { sftp, temporary_path, target_path, backup_path } => {
            remove_remote_file_if_present(&sftp, &temporary_path).await;
            let backup_exists = sftp.lock().await.metadata(backup_path.clone()).await.is_ok();
            if backup_exists {
                let target_exists = sftp.lock().await.metadata(target_path.clone()).await.is_ok();
                if target_exists {
                    remove_remote_file_if_present(&sftp, &backup_path).await;
                } else {
                    let _ = sftp.lock().await.rename(backup_path, target_path).await;
                }
            }
        }
        TransferArtifactCleanup::Local { temporary_path, target_path, backup_path } => {
            remove_local_file_if_present(&temporary_path).await;
            if tokio::fs::try_exists(&backup_path).await.unwrap_or(false) {
                if tokio::fs::try_exists(&target_path).await.unwrap_or(false) {
                    remove_local_file_if_present(&backup_path).await;
                } else {
                    let _ = tokio::fs::rename(backup_path, target_path).await;
                }
            }
        }
    }
}

async fn commit_remote_file(
    sftp: &Arc<Mutex<SftpSession>>,
    temporary_path: &str,
    target_path: &str,
    backup_path: &str,
) -> Result<(), String> {
    let target_exists = sftp.lock().await.metadata(target_path.to_string()).await.is_ok();
    if target_exists {
        sftp.lock().await.rename(target_path.to_string(), backup_path.to_string()).await.map_err(sftp_error)?;
    }
    if let Err(error) = sftp.lock().await.rename(temporary_path.to_string(), target_path.to_string()).await {
        if target_exists {
            let _ = sftp.lock().await.rename(backup_path.to_string(), target_path.to_string()).await;
        }
        remove_remote_file_if_present(sftp, temporary_path).await;
        return Err(sftp_error(error));
    }
    if target_exists {
        remove_remote_file_if_present(sftp, backup_path).await;
    }
    Ok(())
}

fn local_transfer_paths(target_path: &Path, task_id: &str) -> Result<(PathBuf, PathBuf), String> {
    let parent = target_path.parent().ok_or_else(|| "Download target has no parent directory".to_string())?;
    Ok((parent.join(format!(".dbx-download-{task_id}.part")), parent.join(format!(".dbx-download-{task_id}.backup"))))
}

async fn remove_local_file_if_present(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
}

async fn commit_local_file(temporary_path: &Path, target_path: &Path, backup_path: &Path) -> Result<(), String> {
    let target_exists = tokio::fs::try_exists(target_path)
        .await
        .map_err(|error| format!("Failed to inspect download target: {error}"))?;
    if target_exists {
        tokio::fs::rename(target_path, backup_path)
            .await
            .map_err(|error| format!("Failed to prepare existing download target: {error}"))?;
    }
    if let Err(error) = tokio::fs::rename(temporary_path, target_path).await {
        if target_exists {
            let _ = tokio::fs::rename(backup_path, target_path).await;
        }
        remove_local_file_if_present(temporary_path).await;
        return Err(format!("Failed to finalize SFTP download: {error}"));
    }
    if target_exists {
        remove_local_file_if_present(backup_path).await;
    }
    Ok(())
}

async fn copy_sftp_chunks<R, W, F>(
    source: &mut R,
    target: &mut W,
    operation: &str,
    cancellation: &CancellationToken,
    mut on_progress: F,
) -> Result<u64, String>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
    F: FnMut(u64),
{
    let mut transferred = 0_u64;
    let mut buffer = vec![0_u8; SFTP_TRANSFER_CHUNK_SIZE];
    loop {
        if cancellation.is_cancelled() {
            return Err("SFTP transfer cancelled".to_string());
        }
        let read = tokio::select! {
            result = source.read(&mut buffer) => {
                result.map_err(|error| format!("SFTP {operation} failed: {error}"))?
            }
            _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
        };
        if read == 0 {
            break;
        }
        tokio::select! {
            result = target.write_all(&buffer[..read]) => {
                result.map_err(|error| format!("SFTP {operation} failed: {error}"))?;
            }
            _ = cancellation.cancelled() => return Err("SFTP transfer cancelled".to_string()),
        }
        transferred = transferred.saturating_add(read as u64);
        on_progress(transferred);
    }
    target.flush().await.map_err(|error| format!("SFTP {operation} failed: {error}"))?;
    Ok(transferred)
}

fn cancel_session_transfers(registrations: &StdMutex<HashMap<String, TransferRegistration>>, session_id: &str) {
    let cancellations: Vec<CancellationToken> = registrations
        .lock()
        .map(|registrations| {
            registrations
                .values()
                .filter(|registration| registration.session_id == session_id)
                .map(|registration| registration.cancellation.clone())
                .collect()
        })
        .unwrap_or_default();
    for cancellation in cancellations {
        cancellation.cancel();
    }
}

impl Default for SshSessionRegistry {
    fn default() -> Self {
        Self::new(PathBuf::new())
    }
}

async fn connect_config(
    config: &ConnectionConfig,
    workbench: &SshWorkbenchConfig,
    known_hosts_path: &Path,
) -> Result<Handle<SshClient>, String> {
    let keepalive_interval = workbench_keepalive_interval(config);
    connect_and_authenticate_with_keepalive(
        &config.host,
        config.port,
        &config.host,
        config.port,
        &config.username,
        &config.password,
        &workbench.key_path,
        &workbench.key_passphrase,
        workbench.use_ssh_agent,
        &workbench.ssh_agent_sock_path,
        &workbench.auth_method,
        config.connect_timeout_secs.max(1),
        known_hosts_path,
        keepalive_interval,
    )
    .await
}

fn workbench_keepalive_interval(config: &ConnectionConfig) -> Option<Duration> {
    (config.keepalive_interval_secs > 0).then(|| Duration::from_secs(config.keepalive_interval_secs))
}

fn ensure_sftp_writable(read_only: bool) -> Result<(), String> {
    if read_only {
        return Err("This SSH connection is read-only; SFTP write operations are disabled".to_string());
    }
    Ok(())
}

async fn publish_frame(
    session_id: &str,
    data: Vec<u8>,
    stream: TerminalStream,
    output_tx: &broadcast::Sender<TerminalFrame>,
    replay: &Mutex<VecDeque<BufferedFrame>>,
    replay_bytes: &Mutex<usize>,
    sequence: &std::sync::atomic::AtomicU64,
) {
    let next = sequence.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let frame = TerminalFrame { session_id: session_id.to_string(), sequence: next, data, stream };
    let bytes = frame.data.len();
    let mut queue = replay.lock().await;
    let mut total = replay_bytes.lock().await;
    queue.push_back(BufferedFrame { frame: frame.clone(), bytes });
    *total += bytes;
    while *total > TERMINAL_BUFFER_LIMIT {
        let Some(removed) = queue.pop_front() else {
            break;
        };
        *total = total.saturating_sub(removed.bytes);
    }
    drop(total);
    drop(queue);
    let _ = output_tx.send(frame);
}

fn sftp_error(error: impl std::fmt::Display) -> String {
    format!("SFTP operation failed: {error}")
}

fn format_permissions(value: u32) -> String {
    format!("{:04o}", value & 0o7777)
}

fn directory_tracking_marker(session_id: &str) -> Vec<u8> {
    format!("\x1b]777;dbx-directory-ready-{session_id}\x07").into_bytes()
}

fn directory_tracking_script(enabled: bool, session_id: &str, shell: RemoteShell) -> String {
    let marker = format!("\\033]777;dbx-directory-ready-{session_id}\\007");
    let (body, history_flush) = match (shell, enabled) {
        (RemoteShell::Bash, true) => (
            r#"if [ -z "${__DBX_CWD_ACTIVE+x}" ]; then __DBX_CWD_ACTIVE=1; __DBX_OLD_HISTCONTROL_SET=${HISTCONTROL+x}; __DBX_OLD_HISTCONTROL=${HISTCONTROL-}; case ":${HISTCONTROL-}:" in *:ignorespace:*|*:ignoreboth:*) __DBX_HISTORY_NEEDS_DELETE=0 ;; *) __DBX_HISTORY_NEEDS_DELETE=1; HISTCONTROL="${HISTCONTROL:+$HISTCONTROL:}ignorespace" ;; esac; __DBX_OLD_PROMPT_COMMAND=${PROMPT_COMMAND-}; __dbx_emit_cwd(){ printf '\033]7;file://%s%s\007' "${HOSTNAME:-localhost}" "$PWD"; }; PROMPT_COMMAND='__dbx_emit_cwd;'"$__DBX_OLD_PROMPT_COMMAND"; if [ "$__DBX_HISTORY_NEEDS_DELETE" = 1 ]; then history -d $((HISTCMD-1)) 2>/dev/null || true; fi; unset __DBX_HISTORY_NEEDS_DELETE; fi"#,
            "",
        ),
        (RemoteShell::Bash, false) => (
            r#"if [ -n "${__DBX_CWD_ACTIVE+x}" ]; then PROMPT_COMMAND=${__DBX_OLD_PROMPT_COMMAND-}; unset -f __dbx_emit_cwd 2>/dev/null || true; if [ "${__DBX_OLD_HISTCONTROL_SET-}" = x ]; then HISTCONTROL=${__DBX_OLD_HISTCONTROL-}; else unset HISTCONTROL; fi; unset __DBX_CWD_ACTIVE __DBX_OLD_PROMPT_COMMAND __DBX_OLD_HISTCONTROL_SET __DBX_OLD_HISTCONTROL; fi"#,
            "",
        ),
        (RemoteShell::Zsh, true) => (
            r#"if [ -z "${__DBX_CWD_ACTIVE+x}" ]; then __DBX_CWD_ACTIVE=1; if [[ -o HIST_IGNORE_SPACE ]]; then __DBX_OLD_HIST_IGNORE_SPACE=1; else __DBX_OLD_HIST_IGNORE_SPACE=0; setopt HIST_IGNORE_SPACE; fi; __dbx_emit_cwd(){ printf '\033]7;file://%s%s\007' "${HOST:-localhost}" "$PWD"; }; typeset -ga precmd_functions; precmd_functions=(__dbx_emit_cwd ${precmd_functions:#__dbx_emit_cwd}); fi"#,
            " \r",
        ),
        (RemoteShell::Zsh, false) => (
            r#"if [ -n "${__DBX_CWD_ACTIVE+x}" ]; then precmd_functions=(${precmd_functions:#__dbx_emit_cwd}); unfunction __dbx_emit_cwd 2>/dev/null || true; if [[ "${__DBX_OLD_HIST_IGNORE_SPACE-1}" = 0 ]]; then unsetopt HIST_IGNORE_SPACE; fi; unset __DBX_CWD_ACTIVE __DBX_OLD_HIST_IGNORE_SPACE; fi"#,
            " \r",
        ),
        (RemoteShell::Other, _) => ("", ""),
    };
    format!(" {body}; printf '{marker}'\r{history_flush}")
}

impl RemoteShell {
    fn supports_directory_tracking(self) -> bool {
        matches!(self, Self::Bash | Self::Zsh)
    }
}

async fn detect_remote_shell(handle: &Handle<SshClient>) -> RemoteShell {
    remote_shell_or_timeout(detect_remote_shell_inner(handle), REMOTE_SHELL_DETECTION_TIMEOUT).await
}

async fn remote_shell_or_timeout<F>(future: F, timeout: Duration) -> RemoteShell
where
    F: Future<Output = RemoteShell>,
{
    tokio::time::timeout(timeout, future).await.unwrap_or(RemoteShell::Other)
}

async fn detect_remote_shell_inner(handle: &Handle<SshClient>) -> RemoteShell {
    let Ok(mut channel) = handle.channel_open_session().await else {
        return RemoteShell::Other;
    };
    if channel.exec(true, "printf '%s' \"$SHELL\"").await.is_err() {
        return RemoteShell::Other;
    }
    let mut output = Vec::new();
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => output.extend_from_slice(&data),
            ChannelMsg::Eof | ChannelMsg::Close => break,
            _ => {}
        }
    }
    classify_remote_shell(&String::from_utf8_lossy(&output))
}

fn classify_remote_shell(value: &str) -> RemoteShell {
    let shell = value.trim().replace('\\', "/");
    match shell.rsplit('/').next().unwrap_or_default() {
        "bash" => RemoteShell::Bash,
        "zsh" => RemoteShell::Zsh,
        _ => RemoteShell::Other,
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn delete_remote(
    sftp: Arc<Mutex<SftpSession>>,
    path: String,
    recursive: bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
    Box::pin(async move {
        // Use LSTAT (symlink_metadata) instead of STAT (metadata) so that symlinks
        // are never followed — a symlink to a directory won't be recursed into,
        // and will be removed via remove_file rather than remove_dir (C-4 fix).
        let metadata = sftp.lock().await.symlink_metadata(path.clone()).await.map_err(sftp_error)?;
        if metadata.file_type().is_dir() {
            if recursive {
                let children: Vec<String> = sftp
                    .lock()
                    .await
                    .read_dir(path.clone())
                    .await
                    .map_err(sftp_error)?
                    .map(|entry| entry.path())
                    .collect();
                for child in children {
                    delete_remote(sftp.clone(), child, true).await?;
                }
            }
            sftp.lock().await.remove_dir(path).await.map_err(sftp_error)
        } else {
            sftp.lock().await.remove_file(path).await.map_err(sftp_error)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        classify_remote_shell, commit_local_file, directory_tracking_marker, directory_tracking_script,
        ensure_sftp_writable, format_permissions, local_transfer_paths, normalize_absolute_remote_path,
        remote_shell_or_timeout, remote_transfer_paths, workbench_keepalive_interval, DirectoryHandshakeFilter,
        RemoteShell, SftpTransferDirection, SftpTransferStatus, SftpTransferTask, SshSessionRegistry,
        SshWorkbenchConfig, TransferArtifactCleanup, TransferCleanupGuard, TransferRegistration,
        DIRECTORY_HANDSHAKE_LIMIT,
    };
    use crate::models::connection::ConnectionConfig;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn decodes_workbench_config_from_external_config() {
        let config: ConnectionConfig = serde_json::from_value(serde_json::json!({
            "id": "ssh-test",
            "name": "SSH Test",
            "db_type": "ssh",
            "host": "127.0.0.1",
            "port": 22,
            "username": "root",
            "password": "",
            "external_config": {
                "sshWorkbench": {
                    "authMethod": "key",
                    "keyPath": "~/.ssh/id_ed25519",
                    "terminalType": "xterm-256color"
                }
            }
        }))
        .expect("SSH connection config");
        let decoded = SshWorkbenchConfig::from_connection(&config);
        assert_eq!(decoded.auth_method, "key");
        assert_eq!(decoded.key_path, "~/.ssh/id_ed25519");
    }

    #[test]
    fn formats_unix_permissions() {
        assert_eq!(format_permissions(0o100755), "0755");
    }

    #[test]
    fn directory_tracking_scripts_are_session_local() {
        let bash_enabled = directory_tracking_script(true, "session-1", RemoteShell::Bash);
        let bash_disabled = directory_tracking_script(false, "session-1", RemoteShell::Bash);
        let zsh_enabled = directory_tracking_script(true, "session-1", RemoteShell::Zsh);
        let zsh_disabled = directory_tracking_script(false, "session-1", RemoteShell::Zsh);
        assert!(bash_enabled.contains("PROMPT_COMMAND"));
        assert!(bash_enabled.contains("dbx-directory-ready-session-1"));
        assert!(bash_disabled.contains("__DBX_OLD_PROMPT_COMMAND"));
        assert!(bash_enabled.starts_with(' '));
        assert!(bash_enabled.contains("HISTCONTROL"));
        assert!(bash_enabled.contains("history -d"));
        assert!(bash_disabled.contains("unset HISTCONTROL"));
        assert!(zsh_enabled.contains("precmd_functions"));
        assert!(zsh_disabled.contains("unfunction __dbx_emit_cwd"));
        assert!(zsh_enabled.starts_with(' '));
        assert!(zsh_enabled.contains("HIST_IGNORE_SPACE"));
        assert!(zsh_enabled.ends_with(" \r"));
        assert!(zsh_disabled.contains("unsetopt HIST_IGNORE_SPACE"));
    }

    #[test]
    fn classifies_supported_remote_shells() {
        assert_eq!(classify_remote_shell("/bin/bash\n"), RemoteShell::Bash);
        assert_eq!(classify_remote_shell("/usr/local/bin/zsh"), RemoteShell::Zsh);
        assert_eq!(classify_remote_shell("/usr/bin/fish"), RemoteShell::Other);
        assert!(RemoteShell::Bash.supports_directory_tracking());
        assert!(RemoteShell::Zsh.supports_directory_tracking());
        assert!(!RemoteShell::Other.supports_directory_tracking());
    }

    #[tokio::test]
    async fn remote_shell_detection_falls_back_after_timeout() {
        let detected =
            remote_shell_or_timeout(std::future::pending::<RemoteShell>(), std::time::Duration::from_millis(1)).await;
        assert_eq!(detected, RemoteShell::Other);
    }

    #[test]
    fn filters_directory_handshake_across_frames() {
        let marker = directory_tracking_marker("session-1");
        let mut filter = DirectoryHandshakeFilter::default();
        filter.begin(marker.clone());
        assert_eq!(filter.filter(b"echoed integration script"), None);
        let split = marker.len() / 2;
        assert_eq!(filter.filter(&marker[..split]), None);
        let mut final_frame = marker[split..].to_vec();
        final_frame.extend_from_slice(b"user@host:~$ ");
        assert_eq!(filter.filter(&final_frame), Some(b"user@host:~$ ".to_vec()));
        assert_eq!(filter.filter(b"pwd\r\n"), Some(b"pwd\r\n".to_vec()));
    }

    #[test]
    fn directory_handshake_fails_open_when_buffer_limit_is_exceeded() {
        let mut filter = DirectoryHandshakeFilter::default();
        filter.begin(directory_tracking_marker("session-1"));
        let echoed = vec![b'x'; DIRECTORY_HANDSHAKE_LIMIT + 1];
        assert_eq!(filter.filter(&echoed), Some(echoed));
        assert!(filter.take_failed());
        assert_eq!(filter.filter(b"terminal remains visible"), Some(b"terminal remains visible".to_vec()));
    }

    #[test]
    fn directory_handshake_fails_open_after_timeout() {
        let mut filter = DirectoryHandshakeFilter::default();
        filter.begin(directory_tracking_marker("session-1"));
        assert_eq!(filter.filter(b"partial handshake"), None);
        filter.started_at = Some(tokio::time::Instant::now() - std::time::Duration::from_secs(4));
        assert_eq!(filter.flush_if_timed_out(), Some(b"partial handshake".to_vec()));
        assert!(filter.take_failed());
    }

    #[test]
    fn read_only_sessions_reject_sftp_mutations() {
        assert!(ensure_sftp_writable(false).is_ok());
        assert!(ensure_sftp_writable(true).unwrap_err().contains("read-only"));
    }

    #[test]
    fn workbench_keepalive_uses_connection_setting() {
        let mut config: ConnectionConfig = serde_json::from_value(serde_json::json!({
            "id": "ssh-test",
            "name": "SSH Test",
            "db_type": "ssh",
            "host": "127.0.0.1",
            "port": 22,
            "username": "root",
            "password": ""
        }))
        .expect("SSH connection config");
        config.keepalive_interval_secs = 45;
        assert_eq!(workbench_keepalive_interval(&config), Some(std::time::Duration::from_secs(45)));
        config.keepalive_interval_secs = 0;
        assert_eq!(workbench_keepalive_interval(&config), None);
    }

    #[tokio::test]
    async fn session_transfer_cancellation_is_scoped_and_owner_checked() {
        let registry = SshSessionRegistry::default();
        let cancellation = tokio_util::sync::CancellationToken::new();
        registry.transfer_cancellations.lock().expect("transfer registry").insert(
            "task-1".to_string(),
            TransferRegistration {
                session_id: "session-1".to_string(),
                owner_session: Some("owner-1".to_string()),
                cancellation: cancellation.clone(),
            },
        );

        assert!(registry.cancel_sftp_transfer_owned("task-1", "owner-2").await.is_err());
        assert!(!cancellation.is_cancelled());
        registry.cancel_session_transfers("session-1").await;
        assert!(cancellation.is_cancelled());
    }

    #[tokio::test]
    async fn dropped_transfer_guard_removes_registration_and_emits_terminal_state() {
        let registry = SshSessionRegistry::default();
        let cancellation = tokio_util::sync::CancellationToken::new();
        let task = SftpTransferTask {
            task_id: "task-1".to_string(),
            session_id: "session-1".to_string(),
            direction: SftpTransferDirection::Upload,
            file_name: "example.txt".to_string(),
            size: 12,
            transferred: 4,
            status: SftpTransferStatus::Running,
            error: None,
            owner_session: Some("owner-1".to_string()),
        };
        registry.transfer_cancellations.lock().expect("transfer registry").insert(
            task.task_id.clone(),
            TransferRegistration {
                session_id: task.session_id.clone(),
                owner_session: task.owner_session.clone(),
                cancellation: cancellation.clone(),
            },
        );
        let mut receiver = registry.subscribe_transfer_progress();
        let guard = TransferCleanupGuard::new(
            task,
            registry.transfer_cancellations.clone(),
            registry.transfer_tx.clone(),
            cancellation,
        );

        drop(guard);
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
            .await
            .expect("terminal transfer event")
            .expect("transfer broadcast");
        assert_eq!(terminal.status, SftpTransferStatus::Failed);
        assert!(terminal.error.as_deref().is_some_and(|error| error.contains("interrupted")));
        assert!(registry.transfer_cancellations.lock().expect("transfer registry").is_empty());
    }

    #[tokio::test]
    async fn dropped_transfer_guard_removes_local_partial_and_restores_backup() {
        let registry = SshSessionRegistry::default();
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("example.txt");
        let temporary = directory.path().join(".dbx-download-task-1.part");
        let backup = directory.path().join(".dbx-download-task-1.backup");
        tokio::fs::write(&temporary, b"partial").await.unwrap();
        tokio::fs::write(&backup, b"original").await.unwrap();

        let cancellation = CancellationToken::new();
        let task = SftpTransferTask {
            task_id: "task-1".to_string(),
            session_id: "session-1".to_string(),
            direction: SftpTransferDirection::Download,
            file_name: "example.txt".to_string(),
            size: 12,
            transferred: 4,
            status: SftpTransferStatus::Running,
            error: None,
            owner_session: None,
        };
        let mut guard = TransferCleanupGuard::new(
            task,
            registry.transfer_cancellations.clone(),
            registry.transfer_tx.clone(),
            cancellation,
        );
        guard.set_artifact_cleanup(TransferArtifactCleanup::Local {
            temporary_path: temporary.clone(),
            target_path: target.clone(),
            backup_path: backup.clone(),
        });
        drop(guard);

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if target.exists() && !temporary.exists() && !backup.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("artifact cleanup");
        assert_eq!(tokio::fs::read(target).await.unwrap(), b"original");
    }

    #[test]
    fn normalizes_remote_paths_without_escaping_remote_root() {
        assert_eq!(normalize_absolute_remote_path("/home//user/./logs").unwrap(), "/home/user/logs");
        assert_eq!(normalize_absolute_remote_path("/home/user/../../etc").unwrap(), "/etc");
        assert_eq!(normalize_absolute_remote_path("/../../tmp").unwrap(), "/tmp");
        assert_eq!(normalize_absolute_remote_path("/").unwrap(), "/");
        assert!(normalize_absolute_remote_path("relative/path").is_err());
    }

    #[test]
    fn creates_remote_transfer_artifacts_next_to_target() {
        let (temporary, backup) = remote_transfer_paths("/home/user/example.txt", "task-1").unwrap();
        assert_eq!(temporary, "/home/user/.dbx-upload-task-1.part");
        assert_eq!(backup, "/home/user/.dbx-upload-task-1.backup");
        assert!(remote_transfer_paths("/", "task-1").is_err());
    }

    #[tokio::test]
    async fn local_download_commit_replaces_target_and_removes_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("example.txt");
        tokio::fs::write(&target, b"old").await.unwrap();
        let (temporary, backup) = local_transfer_paths(&target, "task-1").unwrap();
        tokio::fs::write(&temporary, b"new").await.unwrap();

        commit_local_file(&temporary, &target, &backup).await.unwrap();

        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"new");
        assert!(!temporary.exists());
        assert!(!backup.exists());
    }

    #[test]
    fn transfer_owner_is_not_exposed_on_the_wire() {
        let task = SftpTransferTask {
            task_id: "task-1".to_string(),
            session_id: "session-1".to_string(),
            direction: SftpTransferDirection::Download,
            file_name: "example.txt".to_string(),
            size: 12,
            transferred: 12,
            status: SftpTransferStatus::Completed,
            error: None,
            owner_session: Some("secret-cookie-token".to_string()),
        };
        let serialized = serde_json::to_value(task).expect("serialize transfer task");
        assert!(serialized.get("ownerSession").is_none());
        assert!(!serialized.to_string().contains("secret-cookie-token"));
    }
}
