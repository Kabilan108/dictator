use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use super::protocol::*;
use crate::utils::format_go_duration;

pub const SERVER_CONNECTION_DEADLINE: Duration = Duration::from_secs(30);

/// Handles daemon commands received over IPC.
pub trait CommandHandler: Send + Sync + 'static {
    fn handle_start(&self) -> Result<()>;
    fn handle_stop(&self) -> Result<()>;
    fn handle_toggle(&self) -> Result<()>;
    fn handle_cancel(&self) -> Result<()>;
    fn get_status(&self) -> StatusData;
}

struct Running {
    accept_task: JoinHandle<()>,
    cancel: CancellationToken,
}

/// IPC server that listens for CLI commands on a unix socket.
pub struct Server {
    socket_path: PathBuf,
    handler: Arc<dyn CommandHandler>,
    running: Mutex<Option<Running>>,
}

impl Server {
    pub fn new(handler: Arc<dyn CommandHandler>) -> Self {
        Self::with_path(handler, PathBuf::from(SOCKET_PATH))
    }

    /// Like [`Server::new`] but listens on a custom socket path.
    pub fn with_path(handler: Arc<dyn CommandHandler>, socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            handler,
            running: Mutex::new(None),
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

        if let Err(err) = std::fs::remove_file(&self.socket_path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            warn!(err = %err, "failed to remove existing socket file");
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        let cancel = CancellationToken::new();
        let handler = Arc::clone(&self.handler);
        let accept_task = tokio::spawn(accept_connections(listener, handler, cancel.clone()));

        *running = Some(Running {
            accept_task,
            cancel,
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
        let _ = state.accept_task.await;

        if let Err(err) = std::fs::remove_file(&self.socket_path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            warn!(err = %err, "failed to remove socket file");
        }

        Ok(())
    }
}

async fn accept_connections(
    listener: UnixListener,
    handler: Arc<dyn CommandHandler>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("accept loop terminated due to context cancellation");
                return;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        debug!("new client connection accepted");
                        let handler = Arc::clone(&handler);
                        tokio::spawn(handle_connection(stream, handler));
                    }
                    Err(err) => {
                        warn!(err = %err, "failed to accept connection");
                    }
                }
            }
        }
    }
}

async fn handle_connection(stream: UnixStream, handler: Arc<dyn CommandHandler>) {
    let result = tokio::time::timeout(SERVER_CONNECTION_DEADLINE, async {
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        let response = match reader.read_line(&mut line).await {
            Ok(_) => match serde_json::from_str::<Command>(&line) {
                Ok(cmd) => {
                    debug!(action = %cmd.action, id = %cmd.id, "received command");
                    // handlers may block briefly (audio thread join, subprocesses)
                    let handler = Arc::clone(&handler);
                    match tokio::task::spawn_blocking(move || {
                        process_command(handler.as_ref(), &cmd)
                    })
                    .await
                    {
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

        send_response(&mut writer, &response).await;
    })
    .await;

    if result.is_err() {
        warn!("client connection timed out");
    }
    debug!("client connection closed");
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

async fn send_response<W: AsyncWriteExt + Unpin>(writer: &mut W, response: &Response) {
    let mut payload = match serde_json::to_vec(response) {
        Ok(payload) => payload,
        Err(err) => {
            error!(err = %err, "failed to encode response");
            return;
        }
    };
    payload.push(b'\n');

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
