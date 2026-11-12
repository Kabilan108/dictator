use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use dictator::ipc::client::MAX_RESPONSE_BYTES;
use dictator::ipc::server::MAX_COMMAND_BYTES;
use dictator::ipc::{
    ACTION_CANCEL, ACTION_MIC, ACTION_PROVIDERS, ACTION_START, ACTION_STATUS, ACTION_STOP,
    ACTION_TOGGLE, ACTION_USE_PROVIDER, Client, CommandHandler, DATA_KEY_LAST_ERROR,
    DATA_KEY_MICROPHONE, DATA_KEY_PROVIDER, DATA_KEY_PROVIDERS, DATA_KEY_RECORDING_DURATION,
    DATA_KEY_STATE, DATA_KEY_UPTIME, DaemonState, ERR_INVALID_COMMAND, LEGACY_SOCKET_PATH,
    MIC_RELEASE, MIC_RESUME, ProviderSummary, Server, StatusData, decode_providers,
    default_socket_path,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

struct FakeHandler {
    starts: AtomicUsize,
}

impl CommandHandler for FakeHandler {
    fn handle_start(&self, _received_at: Instant) -> Result<()> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn handle_stop(&self, _received_at: Instant) -> Result<()> {
        bail!("not currently recording")
    }
    fn handle_toggle(&self, _received_at: Instant) -> Result<()> {
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
    fn handle_start(&self, _received_at: Instant) -> Result<()> {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Ok(())
    }

    fn handle_stop(&self, _received_at: Instant) -> Result<()> {
        Ok(())
    }

    fn handle_toggle(&self, _received_at: Instant) -> Result<()> {
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
    fn handle_start(&self, _received_at: Instant) -> Result<()> {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Ok(())
    }

    fn handle_stop(&self, _received_at: Instant) -> Result<()> {
        Ok(())
    }

    fn handle_toggle(&self, _received_at: Instant) -> Result<()> {
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

struct ArrivalHandler {
    release: Mutex<std_mpsc::Receiver<()>>,
    arrivals: Mutex<Vec<Instant>>,
    mic_actions: Mutex<Vec<String>>,
}

impl CommandHandler for ArrivalHandler {
    fn handle_start(&self, received_at: Instant) -> Result<()> {
        self.release.lock().unwrap().recv().unwrap();
        self.arrivals.lock().unwrap().push(received_at);
        Ok(())
    }
    fn handle_stop(&self, _received_at: Instant) -> Result<()> {
        Ok(())
    }
    fn handle_toggle(&self, _received_at: Instant) -> Result<()> {
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
    fn handle_mic(&self, action: &str) -> Result<String> {
        self.mic_actions.lock().unwrap().push(action.to_owned());
        Ok(format!("{action}d"))
    }
    fn get_microphone_status(&self) -> Option<String> {
        Some("warm (Desk microphone)".into())
    }
}

#[tokio::test]
async fn commands_carry_their_arrival_time_and_mic_control_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("dictator.sock");
    let (release_tx, release_rx) = std_mpsc::channel();
    let handler = Arc::new(ArrivalHandler {
        release: Mutex::new(release_rx),
        arrivals: Mutex::new(Vec::new()),
        mic_actions: Mutex::new(Vec::new()),
    });
    let server = Server::with_path(handler.clone(), socket_path.clone());
    server.start().await.unwrap();

    let sent_at = Instant::now();
    let start_path = socket_path.clone();
    let start = tokio::spawn(async move {
        Client::with_path(start_path)
            .send_command(ACTION_START, &[])
            .await
    });
    // The handler is stuck behind other work, as when a previous command
    // still holds the daemon's command lock.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let released_at = Instant::now();
    release_tx.send(()).unwrap();
    assert!(start.await.unwrap().unwrap().success);
    let arrival = handler.arrivals.lock().unwrap()[0];
    assert!(arrival >= sent_at);
    assert!(
        released_at.duration_since(arrival) > Duration::from_millis(200),
        "arrival time was taken when the handler ran"
    );

    let client = Client::with_path(socket_path);
    let resp = client
        .send_command(ACTION_MIC, &[MIC_RELEASE.into()])
        .await
        .unwrap();
    assert!(resp.success, "{}", resp.error);
    assert_eq!(resp.data[DATA_KEY_MICROPHONE], "released");
    let resp = client
        .send_command(ACTION_MIC, &[MIC_RESUME.into()])
        .await
        .unwrap();
    assert_eq!(resp.data[DATA_KEY_MICROPHONE], "resumed");
    for args in [
        vec![],
        vec!["open".to_string()],
        vec![MIC_RELEASE.into(), MIC_RESUME.into()],
    ] {
        let resp = client.send_command(ACTION_MIC, &args).await.unwrap();
        assert!(!resp.success);
        assert!(
            resp.error.contains("mic expects one argument"),
            "{}",
            resp.error
        );
    }
    assert_eq!(
        *handler.mic_actions.lock().unwrap(),
        [MIC_RELEASE, MIC_RESUME]
    );

    let resp = client.status().await.unwrap();
    assert_eq!(resp.data[DATA_KEY_MICROPHONE], "warm (Desk microphone)");
    server.stop().await.unwrap();
}

struct ProviderHandler {
    providers: dictator::provider::ProviderSelector,
}

impl CommandHandler for ProviderHandler {
    fn handle_start(&self, _received_at: Instant) -> Result<()> {
        Ok(())
    }
    fn handle_stop(&self, _received_at: Instant) -> Result<()> {
        Ok(())
    }
    fn handle_toggle(&self, _received_at: Instant) -> Result<()> {
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
    fn list_providers(&self) -> Result<Vec<ProviderSummary>> {
        Ok(self.providers.list())
    }
    fn use_provider(&self, name: &str) -> Result<Vec<ProviderSummary>> {
        self.providers.select(name)
    }
    fn get_active_provider(&self) -> Option<String> {
        Some(self.providers.active())
    }
}

#[tokio::test]
async fn providers_list_and_switch_over_ipc() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("dictator.sock");
    let runtime_path = dir.path().join("runtime.json");
    let mut api = dictator::utils::default_config().api;
    api.providers.get_mut("openai").unwrap().key = "sk-test-secret".into();
    api.providers.insert(
        "siren".into(),
        dictator::utils::Provider {
            endpoint: "http://siren:8000".into(),
            key: "siren-secret".into(),
            model: "large-v3".into(),
            send_vocabulary: true,
        },
    );
    api.active_provider = "siren".into();
    let handler = Arc::new(ProviderHandler {
        providers: dictator::provider::ProviderSelector::load(&api, runtime_path.clone()),
    });
    let server = Server::with_path(handler, socket_path.clone());
    server.start().await.unwrap();
    let client = Client::with_path(socket_path);

    let raw = client.send_command(ACTION_PROVIDERS, &[]).await.unwrap();
    assert!(raw.success);
    assert_eq!(raw.data[DATA_KEY_PROVIDER], "siren");
    assert!(!raw.data[DATA_KEY_PROVIDERS].contains("secret"));
    let listed = decode_providers(&raw).unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|p| (p.name.as_str(), p.active, p.is_default))
            .collect::<Vec<_>>(),
        [("openai", false, false), ("siren", true, true)]
    );

    let switched = client.use_provider("openai").await.unwrap();
    assert!(switched.iter().any(|p| p.name == "openai" && p.active));
    assert!(runtime_path.exists());
    let status = client.status().await.unwrap();
    assert_eq!(status.data[DATA_KEY_PROVIDER], "openai");

    let unknown = client.use_provider("nope").await.unwrap_err().to_string();
    assert_eq!(
        unknown,
        "unknown provider 'nope'; configured providers: openai, siren"
    );
    for (action, args) in [
        (ACTION_USE_PROVIDER, vec![]),
        (ACTION_USE_PROVIDER, vec!["a".to_string(), "b".to_string()]),
        (ACTION_PROVIDERS, vec!["extra".to_string()]),
    ] {
        let response = client.send_command(action, &args).await.unwrap();
        assert!(!response.success, "{action} {args:?}");
    }

    client.use_provider("siren").await.unwrap();
    assert!(!runtime_path.exists());
    server.stop().await.unwrap();
}
