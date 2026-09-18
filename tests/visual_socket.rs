#![allow(clippy::await_holding_lock)]

use std::sync::{Arc, Mutex, MutexGuard};
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

#[test]
fn default_socket_path_fallback_is_user_scoped() {
    let _guard = lock_env();
    // SAFETY: guarded by ENV_LOCK.
    unsafe {
        std::env::set_var("XDG_RUNTIME_DIR", "");
        std::env::set_var("USER", "dictator-test");
    }
    let path = default_socket_path();
    assert!(
        path.ends_with("dictator-osd-dictator-test/osd.sock"),
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
