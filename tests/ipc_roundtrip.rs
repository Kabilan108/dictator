use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use std::time::Duration;

use anyhow::{Result, bail};
use dictator::ipc::client::MAX_RESPONSE_BYTES;
use dictator::ipc::server::MAX_COMMAND_BYTES;
use dictator::ipc::{
    ACTION_CANCEL, ACTION_START, ACTION_STATUS, ACTION_STOP, ACTION_TOGGLE, Client, CommandHandler,
    DATA_KEY_LAST_ERROR, DATA_KEY_RECORDING_DURATION, DATA_KEY_STATE, DATA_KEY_UPTIME, DaemonState,
    ERR_INVALID_COMMAND, LEGACY_SOCKET_PATH, Server, StatusData, default_socket_path,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

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

#[tokio::test]
async fn default_socket_path_is_private_and_shared_by_client_and_server() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let original_runtime_dir = std::env::var_os("XDG_RUNTIME_DIR");
    // SAFETY: no other test in this integration-test process reads this variable.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", dir.path()) };
    let server = Server::new(Arc::new(FakeHandler {
        starts: AtomicUsize::new(0),
    }));
    let client = Client::new();
    let expected = dir.path().join("dictator").join("dictator.sock");
    assert_eq!(default_socket_path(), expected);
    match original_runtime_dir {
        Some(value) => unsafe { std::env::set_var("XDG_RUNTIME_DIR", value) },
        None => unsafe { std::env::remove_var("XDG_RUNTIME_DIR") },
    }

    assert_eq!(client.socket_path(), expected);
    assert_eq!(server.socket_path(), expected);
    assert_ne!(
        client.socket_path(),
        std::path::Path::new(LEGACY_SOCKET_PATH)
    );

    server.start().await.unwrap();
    assert!(client.is_connected().await);
    let mode = std::fs::metadata(expected.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn server_refuses_to_replace_an_active_socket() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("dictator.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let server = Server::with_path(
        Arc::new(FakeHandler {
            starts: AtomicUsize::new(0),
        }),
        socket_path.clone(),
    );

    assert!(server.start().await.is_err());
    assert!(socket_path.exists());
    assert!(UnixStream::connect(&socket_path).await.is_ok());
    drop(listener);
}

#[tokio::test]
async fn server_preserves_a_non_socket_at_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("dictator.sock");
    std::fs::write(&socket_path, b"keep me").unwrap();
    let server = Server::with_path(
        Arc::new(FakeHandler {
            starts: AtomicUsize::new(0),
        }),
        socket_path.clone(),
    );

    assert!(server.start().await.is_err());
    assert_eq!(std::fs::read(&socket_path).unwrap(), b"keep me");
}

#[tokio::test]
async fn server_replaces_a_stale_socket() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("dictator.sock");
    let stale = UnixListener::bind(&socket_path).unwrap();
    drop(stale);
    let server = Server::with_path(
        Arc::new(FakeHandler {
            starts: AtomicUsize::new(0),
        }),
        socket_path.clone(),
    );

    server.start().await.unwrap();
    assert!(Client::with_path(socket_path).is_connected().await);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn server_rejects_oversized_commands() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("dictator.sock");
    let server = Server::with_path(
        Arc::new(FakeHandler {
            starts: AtomicUsize::new(0),
        }),
        socket_path.clone(),
    );
    server.start().await.unwrap();

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    writer
        .write_all(&vec![b'x'; MAX_COMMAND_BYTES + 1])
        .await
        .unwrap();
    let mut line = String::new();
    BufReader::new(reader).read_line(&mut line).await.unwrap();
    let response: dictator::ipc::Response = serde_json::from_str(&line).unwrap();
    assert!(!response.success);
    assert_eq!(response.error, ERR_INVALID_COMMAND);

    server.stop().await.unwrap();
}

#[tokio::test]
async fn client_rejects_oversized_responses() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("dictator.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        {
            let mut command = Vec::new();
            BufReader::new(&mut stream)
                .read_until(b'\n', &mut command)
                .await
                .unwrap();
        }
        stream
            .write_all(&vec![b'x'; MAX_RESPONSE_BYTES + 1])
            .await
            .unwrap();
    });

    let err = Client::with_path(socket_path)
        .status()
        .await
        .expect_err("oversized response must fail");
    assert!(err.to_string().contains("response exceeds"), "{err:#}");
    server.await.unwrap();
}

struct BlockingHandler {
    entered: std_mpsc::Sender<()>,
    release: Mutex<std_mpsc::Receiver<()>>,
}

impl CommandHandler for BlockingHandler {
    fn handle_start(&self) -> Result<()> {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Ok(())
    }

    fn handle_stop(&self) -> Result<()> {
        Ok(())
    }

    fn handle_toggle(&self) -> Result<()> {
        Ok(())
    }

    fn handle_cancel(&self) -> Result<()> {
        Ok(())
    }

    fn get_status(&self) -> StatusData {
        StatusData {
            state: DaemonState::Idle,
            recording_duration: None,
            last_error: None,
            uptime: Duration::ZERO,
        }
    }
}

#[tokio::test]
async fn stop_waits_for_an_in_flight_handler() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("dictator.sock");
    let (entered_tx, entered_rx) = std_mpsc::channel();
    let (release_tx, release_rx) = std_mpsc::channel();
    let server = Arc::new(Server::with_path(
        Arc::new(BlockingHandler {
            entered: entered_tx,
            release: Mutex::new(release_rx),
        }),
        socket_path.clone(),
    ));
    server.start().await.unwrap();

    let request = tokio::spawn(async move { Client::with_path(socket_path).start().await });
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(1)))
        .await
        .unwrap()
        .unwrap();

    let stopping_server = Arc::clone(&server);
    let stop = tokio::spawn(async move { stopping_server.stop().await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !stop.is_finished(),
        "stop returned while a handler was running"
    );

    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), stop)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let _ = request.await.unwrap();
}

#[tokio::test]
async fn stop_does_not_unlink_a_replacement_socket() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("dictator.sock");
    let server = Server::with_path(
        Arc::new(FakeHandler {
            starts: AtomicUsize::new(0),
        }),
        socket_path.clone(),
    );
    server.start().await.unwrap();

    std::fs::remove_file(&socket_path).unwrap();
    let replacement = UnixListener::bind(&socket_path).unwrap();
    assert!(server.stop().await.is_err());

    assert!(UnixStream::connect(&socket_path).await.is_ok());
    drop(replacement);
}

struct DropAwareHandler {
    entered: std_mpsc::Sender<()>,
    release: Mutex<std_mpsc::Receiver<()>>,
    dropped: std_mpsc::Sender<()>,
}

impl Drop for DropAwareHandler {
    fn drop(&mut self) {
        let _ = self.dropped.send(());
    }
}

impl CommandHandler for DropAwareHandler {
    fn handle_start(&self) -> Result<()> {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Ok(())
    }

    fn handle_stop(&self) -> Result<()> {
        Ok(())
    }

    fn handle_toggle(&self) -> Result<()> {
        Ok(())
    }

    fn handle_cancel(&self) -> Result<()> {
        Ok(())
    }

    fn get_status(&self) -> StatusData {
        StatusData {
            state: DaemonState::Idle,
            recording_duration: None,
            last_error: None,
            uptime: Duration::ZERO,
        }
    }
}

#[tokio::test]
async fn dropping_server_cancels_and_drains_its_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("dictator.sock");
    let (entered_tx, entered_rx) = std_mpsc::channel();
    let (release_tx, release_rx) = std_mpsc::channel();
    let (dropped_tx, dropped_rx) = std_mpsc::channel();
    let server = Server::with_path(
        Arc::new(DropAwareHandler {
            entered: entered_tx,
            release: Mutex::new(release_rx),
            dropped: dropped_tx,
        }),
        socket_path.clone(),
    );
    server.start().await.unwrap();

    let request_path = socket_path.clone();
    let request = tokio::spawn(async move { Client::with_path(request_path).start().await });
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(1)))
        .await
        .unwrap()
        .unwrap();

    drop(server);
    assert!(!socket_path.exists());
    release_tx.send(()).unwrap();
    tokio::task::spawn_blocking(move || dropped_rx.recv_timeout(Duration::from_secs(1)))
        .await
        .unwrap()
        .expect("server task retained its handler after cancellation");
    let _ = request.await.unwrap();
}
