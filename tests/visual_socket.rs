#![allow(clippy::await_holding_lock)]

use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::{Arc, Mutex, MutexGuard, mpsc as std_mpsc};
use std::time::Duration;

use dictator::visual::{
    Event, MAX_CLIENTS, SocketSink, StateValue, default_socket_path, new_meter_event,
    new_state_event,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;

// env mutation is process-global; serialize tests that touch XDG_RUNTIME_DIR
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_runtime_dir(dir: &std::path::Path) {
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    // SAFETY: guarded by ENV_LOCK; tests in this file are the only writers.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", dir) };
}

async fn read_line(reader: &mut BufReader<UnixStream>, timeout: Duration) -> Option<String> {
    let mut line = String::new();
    match tokio::time::timeout(timeout, reader.read_line(&mut line)).await {
        Ok(Ok(n)) if n > 0 => Some(line),
        _ => None,
    }
}

#[tokio::test]
async fn socket_sink_sends_snapshot_and_events() {
    let _guard = lock_env();
    let dir = tempfile::tempdir().unwrap();
    set_runtime_dir(dir.path());

    let duration = Duration::from_millis(1234);
    let sink = SocketSink::new(Some(Arc::new(move || {
        new_state_event(StateValue::Recording, Some(duration), "")
    })))
    .unwrap();

    let conn = UnixStream::connect(default_socket_path()).await.unwrap();
    let mut reader = BufReader::new(conn);

    let line = read_line(&mut reader, Duration::from_secs(1))
        .await
        .expect("snapshot line");
    let snapshot: Event = serde_json::from_str(&line).unwrap();
    match snapshot {
        Event::State(state) => {
            assert_eq!(state.value, StateValue::Recording);
            assert_eq!(state.recording_duration_ms, Some(1234));
        }
        other => panic!("expected state snapshot, got {other:?}"),
    }

    sink.publish(new_meter_event(0.25, 0.5).into());

    let line = read_line(&mut reader, Duration::from_secs(1))
        .await
        .expect("meter line");
    let meter: Event = serde_json::from_str(&line).unwrap();
    match meter {
        Event::Meter(meter) => {
            assert_eq!(meter.rms, 0.25);
            assert_eq!(meter.peak, 0.5);
        }
        other => panic!("expected meter event, got {other:?}"),
    }

    sink.close().await.unwrap();
    assert!(!default_socket_path().exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_registration_does_not_lose_a_concurrent_state_publish() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket_path = dir.path().join("dictator").join("osd.sock");

    let state = Arc::new(Mutex::new(StateValue::Idle));
    let (snapshot_started_tx, snapshot_started_rx) = std_mpsc::channel();
    let (release_snapshot_tx, release_snapshot_rx) = std_mpsc::channel();
    let release_snapshot_rx = Arc::new(Mutex::new(release_snapshot_rx));
    let first_snapshot = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let snapshot_state = Arc::clone(&state);
    let snapshot_release = Arc::clone(&release_snapshot_rx);
    let snapshot_is_first = Arc::clone(&first_snapshot);
    let snapshot = Arc::new(move || {
        let value = *snapshot_state.lock().unwrap();
        if snapshot_is_first.swap(false, std::sync::atomic::Ordering::SeqCst) {
            snapshot_started_tx.send(()).unwrap();
            snapshot_release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(1))
                .expect("test did not release the first snapshot");
        }
        new_state_event(value, None, "")
    });
    let sink = SocketSink::with_path(Some(snapshot), socket_path.clone()).unwrap();

    let conn = UnixStream::connect(&socket_path).await.unwrap();
    snapshot_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("client registration did not start its snapshot");

    *state.lock().unwrap() = StateValue::Recording;
    sink.publish(new_state_event(StateValue::Recording, None, "").into());
    release_snapshot_tx.send(()).unwrap();

    let mut reader = BufReader::new(conn);
    let line = read_line(&mut reader, Duration::from_secs(1))
        .await
        .expect("registered client did not receive a state snapshot");
    let event: Event = serde_json::from_str(&line).unwrap();
    match event {
        Event::State(state) => assert_eq!(state.value, StateValue::Recording),
        other => panic!("expected state snapshot, got {other:?}"),
    }

    sink.close().await.unwrap();
}

#[tokio::test]
async fn reentrant_snapshot_publish_has_a_bounded_registration_retry() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket_path = dir.path().join("dictator").join("osd.sock");

    let sink_slot: Arc<Mutex<std::sync::Weak<SocketSink>>> =
        Arc::new(Mutex::new(std::sync::Weak::new()));
    let snapshot_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_sink = Arc::clone(&sink_slot);
    let callback_calls = Arc::clone(&snapshot_calls);
    let snapshot = Arc::new(move || {
        let invocation = callback_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        assert!(
            invocation < 10,
            "snapshot registration retried without a bound"
        );
        let sink = callback_sink.lock().unwrap().upgrade();
        if let Some(sink) = sink {
            sink.publish(new_state_event(StateValue::Recording, None, "").into());
        }
        new_state_event(StateValue::Idle, None, "")
    });
    let sink = SocketSink::with_path(Some(snapshot), socket_path.clone()).unwrap();
    *sink_slot.lock().unwrap() = Arc::downgrade(&sink);

    let conn = UnixStream::connect(&socket_path).await.unwrap();
    let mut reader = BufReader::new(conn);
    let line = read_line(&mut reader, Duration::from_secs(1))
        .await
        .expect("reentrant snapshot did not finish client registration");
    let event: Event = serde_json::from_str(&line).unwrap();
    match event {
        Event::State(state) => assert_eq!(state.value, StateValue::Recording),
        other => panic!("expected state snapshot, got {other:?}"),
    }
    assert!(snapshot_calls.load(std::sync::atomic::Ordering::SeqCst) > 1);

    sink.close().await.unwrap();
}

#[test]
fn default_socket_path_fallback_is_uid_scoped() {
    let _guard = lock_env();
    // SAFETY: guarded by ENV_LOCK.
    unsafe {
        std::env::set_var("XDG_RUNTIME_DIR", "");
    }
    let path = default_socket_path();
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    assert!(
        path.ends_with(format!("dictator-{uid}/osd.sock")),
        "path = {}",
        path.display()
    );
}

#[test]
fn socket_sink_requires_a_tokio_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let result = SocketSink::with_path(None, dir.path().join("dictator").join("osd.sock"));
    let err = match result {
        Ok(_) => panic!("sink creation outside a runtime must fail"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("Tokio runtime"));
}

#[tokio::test]
async fn socket_sink_replaces_a_stale_socket() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket_path = dir.path().join("dictator").join("osd.sock");
    std::fs::create_dir_all(socket_path.parent().unwrap()).unwrap();
    let stale = tokio::net::UnixListener::bind(&socket_path).unwrap();
    drop(stale);

    let sink = SocketSink::with_path(None, socket_path.clone()).unwrap();
    let conn = UnixStream::connect(&socket_path).await.unwrap();
    let mut reader = BufReader::new(conn);
    read_line(&mut reader, Duration::from_secs(1))
        .await
        .expect("snapshot from replacement listener");
    sink.close().await.unwrap();
}

#[tokio::test]
async fn socket_sink_rejects_a_symlinked_private_directory_without_chmod() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let target = root.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
    symlink(&target, root.path().join("dictator")).unwrap();
    let socket_path = root.path().join("dictator").join("osd.sock");

    assert!(SocketSink::with_path(None, socket_path).is_err());
    let mode = std::fs::metadata(target).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "symlink target permissions were changed");
}

#[tokio::test]
async fn socket_sink_does_not_remove_active_socket() {
    let _guard = lock_env();
    let dir = tempfile::tempdir().unwrap();
    set_runtime_dir(dir.path());

    let socket_path = default_socket_path();
    std::fs::create_dir_all(socket_path.parent().unwrap()).unwrap();
    let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();

    let result = SocketSink::new(None);
    assert!(
        result.is_err(),
        "SocketSink::new succeeded with active socket"
    );

    // the active socket must still be usable
    let conn = UnixStream::connect(&socket_path).await;
    assert!(conn.is_ok(), "active socket was removed or broken");
    drop(listener);
}

#[tokio::test]
async fn socket_sink_rejects_clients_over_limit() {
    let _guard = lock_env();
    let dir = tempfile::tempdir().unwrap();
    set_runtime_dir(dir.path());

    let sink = SocketSink::new(None).unwrap();

    let mut conns = Vec::new();
    for _ in 0..MAX_CLIENTS {
        let conn = UnixStream::connect(default_socket_path()).await.unwrap();
        let mut reader = BufReader::new(conn);
        read_line(&mut reader, Duration::from_secs(1))
            .await
            .expect("snapshot for accepted client");
        conns.push(reader);
    }

    let extra = UnixStream::connect(default_socket_path()).await.unwrap();
    let mut reader = BufReader::new(extra);
    let line = read_line(&mut reader, Duration::from_millis(250)).await;
    assert!(
        line.is_none(),
        "extra client received a snapshot, want rejected connection"
    );

    sink.close().await.unwrap();
}

#[tokio::test]
async fn socket_sink_reclaims_an_idle_client_after_eof() {
    let _guard = lock_env();
    let dir = tempfile::tempdir().unwrap();
    set_runtime_dir(dir.path());

    let sink = SocketSink::new(None).unwrap();
    let mut clients = Vec::new();
    for _ in 0..MAX_CLIENTS {
        let conn = UnixStream::connect(default_socket_path()).await.unwrap();
        let mut reader = BufReader::new(conn);
        read_line(&mut reader, Duration::from_secs(1))
            .await
            .expect("snapshot for accepted client");
        clients.push(reader);
    }
    drop(clients.pop());

    let mut replacement = None;
    for _ in 0..20 {
        let conn = UnixStream::connect(default_socket_path()).await.unwrap();
        let mut reader = BufReader::new(conn);
        if read_line(&mut reader, Duration::from_millis(100))
            .await
            .is_some()
        {
            replacement = Some(reader);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        replacement.is_some(),
        "an EOF client continued to occupy a slot"
    );

    sink.close().await.unwrap();
}

#[tokio::test]
async fn socket_sink_drops_slow_clients_when_reliable_queue_fills() {
    let _guard = lock_env();
    let dir = tempfile::tempdir().unwrap();
    set_runtime_dir(dir.path());

    let sink = SocketSink::new(None).unwrap();
    let mut clients = Vec::new();
    for _ in 0..MAX_CLIENTS {
        let conn = UnixStream::connect(default_socket_path()).await.unwrap();
        let mut reader = BufReader::new(conn);
        read_line(&mut reader, Duration::from_secs(1))
            .await
            .expect("snapshot for slow client");
        clients.push(reader);
    }

    let message = "x".repeat(256 * 1024);
    for _ in 0..32 {
        sink.publish(new_state_event(StateValue::Recording, None, &message).into());
    }

    let replacement = UnixStream::connect(default_socket_path()).await.unwrap();
    let mut reader = BufReader::new(replacement);
    read_line(&mut reader, Duration::from_secs(1))
        .await
        .expect("snapshot after slow clients were dropped");

    tokio::time::timeout(Duration::from_secs(2), sink.close())
        .await
        .expect("sink close timed out")
        .unwrap();
}

#[tokio::test]
async fn dropping_socket_sink_releases_listener_and_socket_path() {
    let _guard = lock_env();
    let dir = tempfile::tempdir().unwrap();
    set_runtime_dir(dir.path());

    let sink = SocketSink::new(None).unwrap();
    let socket_path = sink.socket_path().to_path_buf();
    drop(sink);

    tokio::time::timeout(Duration::from_secs(1), async {
        while socket_path.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropping the sink left its socket path behind");
    assert!(UnixStream::connect(socket_path).await.is_err());
}

#[tokio::test]
async fn close_does_not_unlink_a_replacement_socket() {
    let _guard = lock_env();
    let dir = tempfile::tempdir().unwrap();
    set_runtime_dir(dir.path());

    let sink = SocketSink::new(None).unwrap();
    let socket_path = sink.socket_path().to_path_buf();
    std::fs::remove_file(&socket_path).unwrap();
    let replacement = tokio::net::UnixListener::bind(&socket_path).unwrap();

    assert!(sink.close().await.is_err());
    assert!(UnixStream::connect(&socket_path).await.is_ok());
    drop(replacement);
}
