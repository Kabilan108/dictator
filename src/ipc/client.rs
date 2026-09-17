use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{debug, error};

use super::protocol::*;

pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);

pub struct Client {
    socket_path: PathBuf,
    timeout: Duration,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self::with_path(PathBuf::from(SOCKET_PATH))
    }

    /// Like [`Client::new`] but connects to a custom socket path.
    pub fn with_path(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            timeout: CLIENT_TIMEOUT,
        }
    }

    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    pub async fn send_command(&self, action: &str, args: &[String]) -> Result<Response> {
        let cmd = Command {
            id: uuid::Uuid::new_v4().to_string(),
            action: action.to_string(),
            args: args.to_vec(),
            timestamp: chrono::Utc::now(),
        };

        debug!(action = %cmd.action, id = %cmd.id, "sending command");

        tokio::time::timeout(self.timeout, self.exchange(cmd))
            .await
            .map_err(|_| anyhow!("timed out waiting for daemon response"))?
    }

    async fn exchange(&self, cmd: Command) -> Result<Response> {
        let stream = self
            .connect()
            .await
            .map_err(|e| anyhow!("failed to connect to daemon: {e}"))?;
        let (reader, mut writer) = stream.into_split();

        let mut payload = serde_json::to_vec(&cmd)?;
        payload.push(b'\n');
        if let Err(err) = writer.write_all(&payload).await {
            error!(err = %err, "failed to encode command");
            bail!("failed to send command: {err}");
        }

        let mut line = String::new();
        let mut reader = BufReader::new(reader);
        if let Err(err) = reader.read_line(&mut line).await {
            error!(err = %err, "failed to decode response");
            bail!("failed to receive response: {err}");
        }
        let response: Response = match serde_json::from_str(&line) {
            Ok(response) => response,
            Err(err) => {
                error!(err = %err, "failed to decode response");
                bail!("failed to receive response: {err}");
            }
        };

        if response.id != cmd.id {
            error!(expected = %cmd.id, got = %response.id, "response ID mismatch");
            bail!("response ID mismatch");
        }

        debug!(action = %cmd.action, success = response.success, "received response");
        Ok(response)
    }

    async fn connect(&self) -> Result<UnixStream> {
        match UnixStream::connect(&self.socket_path).await {
            Ok(stream) => {
                debug!(path = %self.socket_path.display(), "connected to daemon");
                Ok(stream)
            }
            Err(err) => {
                error!(err = %err, "failed to dial unix socket");
                Err(err.into())
            }
        }
    }

    pub async fn start(&self) -> Result<Response> {
        self.send_command(ACTION_START, &[]).await
    }

    pub async fn stop(&self) -> Result<Response> {
        self.send_command(ACTION_STOP, &[]).await
    }

    pub async fn toggle(&self) -> Result<Response> {
        self.send_command(ACTION_TOGGLE, &[]).await
    }

    pub async fn cancel(&self) -> Result<Response> {
        self.send_command(ACTION_CANCEL, &[]).await
    }

    pub async fn status(&self) -> Result<Response> {
        self.send_command(ACTION_STATUS, &[]).await
    }

    pub async fn is_connected(&self) -> bool {
        matches!(
            tokio::time::timeout(CONNECTION_TIMEOUT, self.connect()).await,
            Ok(Ok(_))
        )
    }

    /// Waits until the daemon accepts connections, polling every `check_interval`.
    pub async fn wait_for_daemon(&self, check_interval: Duration) -> Result<()> {
        debug!("waiting for daemon to become available");
        let mut ticker = tokio::time::interval(check_interval);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if self.is_connected().await {
                debug!("daemon is now available");
                return Ok(());
            }
        }
    }
}
