use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{debug, error};

use super::protocol::*;
use super::unix_socket::validate_private_socket_parent;

pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub struct Client {
    socket_path: PathBuf,
    timeout: Duration,
    validate_socket_parent: bool,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self {
            socket_path: default_socket_path(),
            timeout: CLIENT_TIMEOUT,
            validate_socket_parent: true,
        }
    }

    /// Like [`Client::new`] but connects to a custom socket path.
    pub fn with_path(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            timeout: CLIENT_TIMEOUT,
            validate_socket_parent: false,
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
            .map_err(|_| {
                anyhow!(
                    "timed out waiting for daemon response; outcome is unknown; the command may \
                     still execute, so do not automatically retry"
                )
            })?
    }

    async fn exchange(&self, cmd: Command) -> Result<Response> {
        let stream = self
            .connect()
            .await
            .map_err(|e| anyhow!("failed to connect to daemon: {e}"))?;
        let (reader, mut writer) = stream.into_split();

        let mut payload = serde_json::to_vec(&cmd)?;
        payload.push(b'\n');
        if payload.len() > super::server::MAX_COMMAND_BYTES {
            bail!("command exceeds {} bytes", super::server::MAX_COMMAND_BYTES);
        }
        if let Err(err) = writer.write_all(&payload).await {
            error!(err = %err, "failed to encode command");
            bail!("failed to send command: {err}");
        }

        let mut line = Vec::new();
        let mut reader = BufReader::new(reader).take(MAX_RESPONSE_BYTES as u64 + 1);
        if let Err(err) = reader.read_until(b'\n', &mut line).await {
            error!(err = %err, "failed to decode response");
            bail!("failed to receive response: {err}");
        }
        if line.len() > MAX_RESPONSE_BYTES {
            bail!("daemon response exceeds {MAX_RESPONSE_BYTES} bytes");
        }
        if line.is_empty() {
            bail!("failed to receive response: daemon closed the connection");
        }
        let response: Response = match serde_json::from_slice(&line) {
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
        if self.validate_socket_parent {
            validate_private_socket_parent(&self.socket_path)?;
        }
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
        if check_interval.is_zero() {
            bail!("check interval must be greater than zero");
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::net::UnixListener;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn zero_poll_interval_returns_an_error() {
        let client = Client::with_path(PathBuf::from("/unused/dictator.sock"));
        let error = client.wait_for_daemon(Duration::ZERO).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "check interval must be greater than zero"
        );
    }

    #[tokio::test]
    async fn timeout_reports_unknown_outcome_after_command_was_received() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("dictator.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let (received_tx, received_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel::<()>();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut line = Vec::new();
            let mut reader = BufReader::new(stream);
            reader.read_until(b'\n', &mut line).await.unwrap();
            let command: Command = serde_json::from_slice(&line).unwrap();
            received_tx.send(command).unwrap();
            let _ = release_rx.await;
        });

        let mut client = Client::with_path(socket_path);
        client.timeout = Duration::from_millis(50);
        let request = tokio::spawn(async move { client.start().await });

        let command = tokio::time::timeout(Duration::from_secs(1), received_rx)
            .await
            .expect("server did not receive command before test timeout")
            .expect("mock server stopped before reporting the command");
        assert_eq!(command.action, ACTION_START);

        let error = request
            .await
            .unwrap()
            .expect_err("client should time out while the server holds its response");
        assert_eq!(
            error.to_string(),
            "timed out waiting for daemon response; outcome is unknown; the command may still \
             execute, so do not automatically retry"
        );

        let _ = release_tx.send(());
        server.await.unwrap();
    }
}
