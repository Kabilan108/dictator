use std::collections::BTreeMap;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use super::protocol::*;
use super::unix_socket::{SocketProbe, ensure_private_socket_parent, probe_socket};
use crate::utils::format_go_duration;

pub const SERVER_CONNECTION_DEADLINE: Duration = Duration::from_secs(30);
pub const MAX_COMMAND_BYTES: usize = 64 * 1024;
pub const MAX_CONNECTIONS: usize = 64;
const SOCKET_FILE_PERM: u32 = 0o600;

#[derive(Clone, Copy)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

/// Handles daemon commands received over IPC.
pub trait CommandHandler: Send + Sync + 'static {
    fn handle_start(&self) -> Result<()>;
    fn handle_stop(&self) -> Result<()>;
    fn handle_toggle(&self) -> Result<()>;
    fn handle_cancel(&self) -> Result<()>;
    fn get_status(&self) -> StatusData;
    fn handle_retry(&self, _recording_id: Option<i64>) -> Result<i64> {
        bail!("retry is not supported")
    }
    fn get_last_recording(&self) -> Option<(i64, u64)> {
        None
    }
    fn get_audio_level(&self) -> Option<(f64, f64)> {
        None
    }
    fn get_recovered_text(&self) -> Option<String> {
        None
    }
}

struct Running {
    accept_task: JoinHandle<()>,
    cancel: CancellationToken,
    socket_identity: SocketIdentity,
}

/// IPC server that listens for CLI commands on a unix socket.
pub struct Server {
    socket_path: PathBuf,
    handler: Arc<dyn CommandHandler>,
    running: Mutex<Option<Running>>,
    secure_socket_parent: bool,
}

impl Server {
    pub fn new(handler: Arc<dyn CommandHandler>) -> Self {
        Self {
            socket_path: default_socket_path(),
            handler,
            running: Mutex::new(None),
            secure_socket_parent: true,
        }
    }

    /// Like [`Server::new`] but listens on a custom socket path.
    pub fn with_path(handler: Arc<dyn CommandHandler>, socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            handler,
            running: Mutex::new(None),
            secure_socket_parent: false,
        }
    }

    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    pub async fn start(&self) -> Result<()> {
        let mut running = self.running.lock().await;

        debug!(path = %self.socket_path.display(), "starting ipc server");

        if running.is_some() {
            bail!("server is already running");
        }

        if self.secure_socket_parent {
            ensure_private_socket_parent(&self.socket_path)?;
        }
        prepare_socket_path(&self.socket_path)?;

        let listener = UnixListener::bind(&self.socket_path)?;
        let socket_identity = match socket_identity(&self.socket_path) {
            Ok(identity) => identity,
            Err(err) => {
                drop(listener);
                return Err(err);
            }
        };
        if let Err(err) = std::fs::set_permissions(
            &self.socket_path,
            std::fs::Permissions::from_mode(SOCKET_FILE_PERM),
        ) {
            drop(listener);
            let _ = remove_socket_if_owned(&self.socket_path, socket_identity);
            return Err(anyhow!("failed to secure IPC socket: {err}"));
        }
        let cancel = CancellationToken::new();
        let handler = Arc::clone(&self.handler);
        let accept_task = tokio::spawn(accept_connections(listener, handler, cancel.clone()));

        *running = Some(Running {
            accept_task,
            cancel,
            socket_identity,
        });
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        let mut running = self.running.lock().await;
        let Some(state) = running.take() else {
            return Ok(());
        };

        debug!("stopping ipc server");
        state.cancel.cancel();
        let task_result = state.accept_task.await;
        let socket_result = remove_socket_if_owned(&self.socket_path, state.socket_identity);

        if let Err(err) = task_result {
            return Err(anyhow!("IPC server task failed during shutdown: {err}"));
        }
        socket_result
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let Some(state) = self.running.get_mut().take() else {
            return;
        };
        state.cancel.cancel();
        // Dropping the handle detaches the task. The cancellation path still owns and
        // joins every connection task, including any running blocking handler.
        drop(state.accept_task);
        if let Err(err) = remove_socket_if_owned(&self.socket_path, state.socket_identity) {
            warn!(err = %err, "failed to remove IPC socket while dropping server");
        }
    }
}

async fn accept_connections(
    listener: UnixListener,
    handler: Arc<dyn CommandHandler>,
    cancel: CancellationToken,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("accept loop terminated due to context cancellation");
                break;
            }
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(err)) = result {
                    error!(err = %err, "IPC connection task panicked");
                }
            }
            accepted = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                match accepted {
                    Ok((stream, _)) => {
                        debug!("new client connection accepted");
                        let handler = Arc::clone(&handler);
                        connections.spawn(handle_connection(stream, handler, cancel.clone()));
                    }
                    Err(err) => {
                        warn!(err = %err, "failed to accept connection");
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                }
            }
        }
    }

    while let Some(result) = connections.join_next().await {
        if let Err(err) = result {
            error!(err = %err, "IPC connection task panicked during shutdown");
        }
    }
}

async fn handle_connection(
    stream: UnixStream,
    handler: Arc<dyn CommandHandler>,
    cancel: CancellationToken,
) {
    let deadline = tokio::time::Instant::now() + SERVER_CONNECTION_DEADLINE;
    let result = async {
        let (reader, mut writer) = stream.into_split();
        let line = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            result = tokio::time::timeout_at(deadline, read_frame(reader, MAX_COMMAND_BYTES)) => {
                match result {
                    Ok(result) => result,
                    Err(_) => return Err(anyhow!("client connection timed out")),
                }
            }
        };

        let response = match line {
            Ok(line) => match serde_json::from_slice::<Command>(&line) {
                Ok(cmd) => {
                    debug!(action = %cmd.action, id = %cmd.id, "received command");
                    // handlers may block briefly (audio thread join, subprocesses)
                    let handler = Arc::clone(&handler);
                    let mut task = tokio::task::spawn_blocking(move || {
                        process_command(handler.as_ref(), &cmd)
                    });
                    let response = tokio::select! {
                        biased;
                        result = &mut task => Some(result),
                        _ = cancel.cancelled() => None,
                        _ = tokio::time::sleep_until(deadline) => None,
                    };

                    // A spawn_blocking task cannot be cancelled once it starts. Always join it
                    // before this connection task finishes so Server::stop fully quiesces handlers.
                    let result = match response {
                        Some(result) => result,
                        None => {
                            let _ = task.await;
                            return if cancel.is_cancelled() {
                                Ok(())
                            } else {
                                Err(anyhow!("client connection timed out"))
                            };
                        }
                    };
                    match result {
                        Ok(response) => response,
                        Err(err) => {
                            error!(err = %err, "command handler panicked");
                            Response {
                                id: String::new(),
                                success: false,
                                error: "internal error".to_string(),
                                data: BTreeMap::new(),
                            }
                        }
                    }
                }
                Err(err) => {
                    error!(err = %err, "failed to decode command");
                    Response {
                        id: String::new(),
                        success: false,
                        error: ERR_INVALID_COMMAND.to_string(),
                        data: BTreeMap::new(),
                    }
                }
            },
            Err(err) => {
                error!(err = %err, "failed to decode command");
                Response {
                    id: String::new(),
                    success: false,
                    error: ERR_INVALID_COMMAND.to_string(),
                    data: BTreeMap::new(),
                }
            }
        };

        tokio::select! {
            biased;
            _ = cancel.cancelled() => {}
            result = tokio::time::timeout_at(deadline, send_response(&mut writer, &response)) => {
                if result.is_err() {
                    return Err(anyhow!("client connection timed out"));
                }
            }
        }
        Ok(())
    }
    .await;

    if let Err(err) = result {
        warn!(err = %err, "IPC client connection failed");
    }
    debug!("client connection closed");
}

async fn read_frame<R>(reader: R, limit: usize) -> Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader).take(limit as u64 + 1);
    let mut frame = Vec::new();
    let read = reader.read_until(b'\n', &mut frame).await?;
    if read == 0 {
        bail!("connection closed before a command was received");
    }
    if frame.len() > limit {
        bail!("command exceeds {limit} bytes");
    }
    Ok(frame)
}

fn process_command(handler: &dyn CommandHandler, cmd: &Command) -> Response {
    let mut response = Response {
        id: cmd.id.clone(),
        success: false,
        error: String::new(),
        data: BTreeMap::new(),
    };

    let mut err: Option<anyhow::Error> = None;

    match cmd.action.as_str() {
        ACTION_START => match handler.handle_start() {
            Ok(()) => {
                response.success = true;
                response
                    .data
                    .insert(DATA_KEY_STATE.into(), DaemonState::Recording.to_string());
            }
            Err(e) => err = Some(e),
        },
        ACTION_STOP => match handler.handle_stop() {
            Ok(()) => {
                response.success = true;
                response
                    .data
                    .insert(DATA_KEY_STATE.into(), DaemonState::Idle.to_string());
            }
            Err(e) => err = Some(e),
        },
        ACTION_TOGGLE => match handler.handle_toggle() {
            Ok(()) => response.success = true,
            Err(e) => err = Some(e),
        },
        ACTION_CANCEL => match handler.handle_cancel() {
            Ok(()) => {
                response.success = true;
                response
                    .data
                    .insert(DATA_KEY_STATE.into(), DaemonState::Idle.to_string());
            }
            Err(e) => err = Some(e),
        },
        ACTION_RETRY => {
            let recording_id = match cmd.args.as_slice() {
                [] => Ok(None),
                [value] => value
                    .parse::<i64>()
                    .map(Some)
                    .map_err(|_| anyhow!("recording id must be an integer")),
                _ => Err(anyhow!("retry accepts at most one recording id")),
            };
            match recording_id.and_then(|id| handler.handle_retry(id)) {
                Ok(recording_id) => {
                    response.success = true;
                    response
                        .data
                        .insert(DATA_KEY_RECORDING_ID.into(), recording_id.to_string());
                    response
                        .data
                        .insert(DATA_KEY_STATE.into(), DaemonState::Transcribing.to_string());
                }
                Err(e) => err = Some(e),
            }
        }
        ACTION_STATUS => {
            let status = handler.get_status();
            response.success = true;
            response
                .data
                .insert(DATA_KEY_STATE.into(), status.state.to_string());
            response
                .data
                .insert(DATA_KEY_UPTIME.into(), format_go_duration(status.uptime));
            if let Some(duration) = status.recording_duration {
                response.data.insert(
                    DATA_KEY_RECORDING_DURATION.into(),
                    format_go_duration(duration),
                );
            }
            if let Some(last_error) = status.last_error {
                response.data.insert(DATA_KEY_LAST_ERROR.into(), last_error);
            }
            if let Some((recording_id, generation)) = handler.get_last_recording() {
                response
                    .data
                    .insert(DATA_KEY_LAST_RECORDING_ID.into(), recording_id.to_string());
                response.data.insert(
                    DATA_KEY_LAST_RECORDING_GENERATION.into(),
                    generation.to_string(),
                );
            }
            if let Some((rms, peak)) = handler.get_audio_level() {
                response
                    .data
                    .insert(DATA_KEY_AUDIO_LEVEL_RMS.into(), rms.to_string());
                response
                    .data
                    .insert(DATA_KEY_AUDIO_LEVEL_PEAK.into(), peak.to_string());
            }
            if let Some(text) = handler.get_recovered_text() {
                response.data.insert(DATA_KEY_TEXT.into(), text);
            }
        }
        other => {
            err = Some(anyhow::anyhow!("unknown action: {other}"));
            response.error = ERR_INVALID_COMMAND.to_string();
        }
    }

    if let Some(e) = err
        && response.error.is_empty()
    {
        response.error = format!("{e:#}");
        error!(err = %response.error, "command failed");
    }

    response
}

async fn send_response<W: AsyncWrite + Unpin>(writer: &mut W, response: &Response) {
    let mut payload = match serde_json::to_vec(response) {
        Ok(payload) => payload,
        Err(err) => {
            error!(err = %err, "failed to encode response");
            return;
        }
    };
    payload.push(b'\n');
    if payload.len() > super::client::MAX_RESPONSE_BYTES {
        let fallback = Response {
            id: response.id.clone(),
            success: false,
            error: "response too large".to_string(),
            data: BTreeMap::new(),
        };
        payload = match serde_json::to_vec(&fallback) {
            Ok(payload) => payload,
            Err(err) => {
                error!(err = %err, "failed to encode fallback response");
                return;
            }
        };
        payload.push(b'\n');
    }

    if let Err(err) = writer.write_all(&payload).await {
        error!(err = %err, "failed to encode response");
        return;
    }

    if response.success {
        debug!(id = %response.id, "sent success response");
    } else {
        debug!(id = %response.id, error = %response.error, "sent error response");
    }
}

fn prepare_socket_path(socket_path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if !metadata.file_type().is_socket() {
        bail!(
            "IPC socket path exists and is not a socket: {}",
            socket_path.display()
        );
    }
    let identity = SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    match probe_socket(socket_path)? {
        SocketProbe::Active => {
            bail!("IPC socket already in use: {}", socket_path.display())
        }
        SocketProbe::Stale => {}
    }
    remove_socket_if_owned(socket_path, identity)
        .map_err(|err| anyhow!("failed to remove stale IPC socket: {err}"))
}

fn socket_identity(socket_path: &Path) -> Result<SocketIdentity> {
    let metadata = std::fs::symlink_metadata(socket_path)?;
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn remove_socket_if_owned(socket_path: &Path, expected: SocketIdentity) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if !metadata.file_type().is_socket()
        || metadata.dev() != expected.device
        || metadata.ino() != expected.inode
    {
        bail!("refusing to remove an IPC socket path no longer owned by this server");
    }
    std::fs::remove_file(socket_path)?;
    Ok(())
}
