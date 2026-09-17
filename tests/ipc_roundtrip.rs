use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use dictator::ipc::{
    ACTION_CANCEL, ACTION_START, ACTION_STATUS, ACTION_STOP, ACTION_TOGGLE, Client, CommandHandler,
    DATA_KEY_LAST_ERROR, DATA_KEY_RECORDING_DURATION, DATA_KEY_STATE, DATA_KEY_UPTIME, DaemonState,
    ERR_INVALID_COMMAND, SOCKET_PATH, Server, StatusData,
};

struct FakeHandler {
    starts: AtomicUsize,
}

impl CommandHandler for FakeHandler {
    fn handle_start(&self) -> Result<()> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn handle_stop(&self) -> Result<()> {
        bail!("not currently recording")
    }
    fn handle_toggle(&self) -> Result<()> {
        Ok(())
    }
    fn handle_cancel(&self) -> Result<()> {
        Ok(())
    }
    fn get_status(&self) -> StatusData {
        StatusData {
            state: DaemonState::Recording,
            recording_duration: Some(Duration::from_millis(1500)),
            last_error: Some("boom".into()),
            uptime: Duration::from_secs(3723),
        }
    }
}

/// Exercises the full client/server exchange over a private socket path so the
/// test never clobbers a live daemon on the default path.
#[tokio::test]
async fn client_server_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("dictator.sock");

    let handler = Arc::new(FakeHandler {
        starts: AtomicUsize::new(0),
    });
    let server = Server::with_path(handler.clone(), socket_path.clone());
    server.start().await.unwrap();

    let client = Client::with_path(socket_path.clone());
    assert!(client.is_connected().await);

    let resp = client.send_command(ACTION_START, &[]).await.unwrap();
    assert!(resp.success);
    assert_eq!(resp.data[DATA_KEY_STATE], "recording");
    assert_eq!(handler.starts.load(Ordering::SeqCst), 1);

    let resp = client.send_command(ACTION_STOP, &[]).await.unwrap();
    assert!(!resp.success);
    assert_eq!(resp.error, "not currently recording");

    let resp = client.send_command(ACTION_TOGGLE, &[]).await.unwrap();
    assert!(resp.success);
    assert!(resp.data.is_empty());

    let resp = client.send_command(ACTION_CANCEL, &[]).await.unwrap();
    assert!(resp.success);
    assert_eq!(resp.data[DATA_KEY_STATE], "idle");

    let resp = client.send_command(ACTION_STATUS, &[]).await.unwrap();
    assert!(resp.success);
    assert_eq!(resp.data[DATA_KEY_STATE], "recording");
    assert_eq!(resp.data[DATA_KEY_UPTIME], "1h2m3s");
    assert_eq!(resp.data[DATA_KEY_RECORDING_DURATION], "1.5s");
    assert_eq!(resp.data[DATA_KEY_LAST_ERROR], "boom");

    let resp = client.send_command("bogus", &[]).await.unwrap();
    assert!(!resp.success);
    assert_eq!(resp.error, ERR_INVALID_COMMAND);

    server.stop().await.unwrap();
    assert!(!socket_path.exists());
    assert!(!client.is_connected().await);
}

#[test]
fn default_socket_path_matches_go() {
    assert_eq!(SOCKET_PATH, "/tmp/dictator.sock");
    assert_eq!(
        Client::new().socket_path(),
        std::path::Path::new(SOCKET_PATH)
    );
}
