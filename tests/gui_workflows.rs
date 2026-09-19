#![cfg(feature = "gui")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use dictator::gui::backend::{Backend, Reply, Request};
use dictator::ipc::{Client, DATA_KEY_TEXT, DaemonState};
use dictator::storage::{Db, HistoryQuery, RecordingStatus, TranscriptionAttempt};

struct TestEnvironment {
    _root: tempfile::TempDir,
    home: PathBuf,
    config_home: PathBuf,
    data_home: PathBuf,
    state_home: PathBuf,
    runtime_dir: PathBuf,
    bin_dir: PathBuf,
}

impl TestEnvironment {
    fn new(endpoint: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let config_home = root.path().join("config");
        let data_home = root.path().join("data");
        let state_home = root.path().join("state");
        let runtime_dir = root.path().join("runtime");
        let bin_dir = root.path().join("bin");
        for path in [
            &home,
            &config_home,
            &data_home,
            &state_home,
            &runtime_dir,
            &bin_dir,
        ] {
            std::fs::create_dir(path).unwrap();
        }
        std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

        for program in ["xclip", "xdotool"] {
            let path = bin_dir.join(program);
            std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }

        let app_config = config_home.join("dictator");
        std::fs::create_dir(&app_config).unwrap();
        let config = serde_json::json!({
            "enable_osd": false,
            "notifications": "off",
            "api": {
                "active_provider": "mock",
                "timeout": 2,
                "providers": {
                    "mock": {
                        "endpoint": endpoint,
                        "key": "integration-test-key",
                        "model": "integration-test-model"
                    }
                }
            },
            "audio": {
                "sample_rate": 16_000,
                "channels": 1,
                "bit_depth": 16,
                "frames_per_block": 1_024,
                "max_duration_min": 5
            }
        });
        std::fs::write(
            app_config.join("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();

        Self {
            _root: root,
            home,
            config_home,
            data_home,
            state_home,
            runtime_dir,
            bin_dir,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dictator"));
        let path = std::env::join_paths(
            std::iter::once(self.bin_dir.clone()).chain(
                std::env::var_os("PATH")
                    .into_iter()
                    .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>()),
            ),
        )
        .unwrap();
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_STATE_HOME", &self.state_home)
            .env("XDG_RUNTIME_DIR", &self.runtime_dir)
            .env("PATH", path);
        if let Some(library_path) = std::env::var_os("LD_LIBRARY_PATH") {
            command.env("LD_LIBRARY_PATH", library_path);
        }
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn activate_for_backend(&self) {
        // SAFETY: this integration-test binary contains one test, and it sets
        // these variables before Dictator's lazy paths or backend thread start.
        unsafe {
            std::env::set_var("HOME", &self.home);
            std::env::set_var("XDG_CONFIG_HOME", &self.config_home);
            std::env::set_var("XDG_DATA_HOME", &self.data_home);
            std::env::set_var("XDG_STATE_HOME", &self.state_home);
            std::env::set_var("XDG_RUNTIME_DIR", &self.runtime_dir);
        }
    }

    fn database_path(&self) -> PathBuf {
        self.data_home.join("dictator").join("app.db")
    }

    fn socket_path(&self) -> PathBuf {
        self.runtime_dir.join("dictator").join("dictator.sock")
    }
}

struct ProviderResponse {
    status: &'static str,
    body: &'static str,
    delay: Duration,
    received: Option<Sender<()>>,
    release: Option<Receiver<()>>,
}

fn spawn_provider(responses: Vec<ProviderResponse>) -> (String, thread::JoinHandle<Vec<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        for response in responses {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "provider was not contacted");
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("provider accept failed: {error}"),
                }
            };
            requests.push(read_request(&mut stream));
            if let Some(received) = response.received {
                received.send(()).unwrap();
            }
            if let Some(release) = response.release {
                release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            thread::sleep(response.delay);
            let _ = write!(
                stream,
                "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.status,
                response.body.len(),
                response.body
            );
        }
        requests
    });
    (endpoint, server)
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    let mut expected_len = None;
    loop {
        let mut chunk = [0u8; 8 * 1024];
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0, "provider request ended before its body");
        request.extend_from_slice(&chunk[..read]);
        if expected_len.is_none()
            && let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
        {
            let header_len = header_end + 4;
            let headers = String::from_utf8_lossy(&request[..header_len]);
            let content_len = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .map(str::parse::<usize>)
                })
                .expect("request did not contain Content-Length")
                .unwrap();
            expected_len = Some(header_len + content_len);
        }
        if expected_len.is_some_and(|len| request.len() >= len) {
            return request;
        }
    }
}

fn pcm_wav(path: &Path) {
    let data_len = 32_000u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.resize(44 + data_len as usize, 0);
    std::fs::write(path, wav).unwrap();
}

fn output_details(output: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_attempt_latency(attempt: &TranscriptionAttempt, minimum: Duration) -> i64 {
    let latency_ms = attempt
        .latency_ms
        .expect("provider latency was not recorded");
    assert!(latency_ms >= minimum.as_millis() as i64);
    assert_eq!(
        attempt
            .finished_at
            .signed_duration_since(attempt.started_at)
            .num_milliseconds(),
        latency_ms,
        "attempt timestamps do not describe the measured latency"
    );
    latency_ms
}

fn next_reply(backend: &Backend) -> Reply {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(reply) = backend.drain().into_iter().next() {
            return reply;
        }
        assert!(Instant::now() < deadline, "GUI backend did not reply");
        thread::sleep(Duration::from_millis(10));
    }
}

struct DaemonGuard(Option<Child>);

impl DaemonGuard {
    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().unwrap()
    }

    fn stop(mut self) -> Output {
        let mut child = self.0.take().unwrap();
        // SAFETY: child.id() belongs to the live daemon process created by this test.
        assert_eq!(
            unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
            0
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                return child.wait_with_output().unwrap();
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn terminal_retry_and_gui_revision_workflow_preserve_recording_identity() {
    let first_delay = Duration::from_millis(80);
    let persistence_failure_delay = Duration::from_millis(90);
    let second_delay = Duration::from_millis(110);
    let recovered_delay = Duration::from_millis(140);
    let (stalled_received_tx, stalled_received_rx) = channel();
    let (release_stalled_tx, release_stalled_rx) = channel();
    let (endpoint, provider) = spawn_provider(vec![
        ProviderResponse {
            status: "503 Service Unavailable",
            body: r#"{"error":"first failure"}"#,
            delay: first_delay,
            received: None,
            release: None,
        },
        ProviderResponse {
            status: "200 OK",
            body: r#"{"text":"recovered but unsaved"}"#,
            delay: persistence_failure_delay,
            received: None,
            release: None,
        },
        ProviderResponse {
            status: "503 Service Unavailable",
            body: r#"{"error":"retry failure"}"#,
            delay: second_delay,
            received: None,
            release: None,
        },
        ProviderResponse {
            status: "200 OK",
            body: r#"{"text":"cancelled words"}"#,
            delay: Duration::ZERO,
            received: Some(stalled_received_tx),
            release: Some(release_stalled_rx),
        },
        ProviderResponse {
            status: "200 OK",
            body: r#"{"text":"recovered words"}"#,
            delay: recovered_delay,
            received: None,
            release: None,
        },
    ]);
    let environment = TestEnvironment::new(&endpoint);
    environment.activate_for_backend();
    let audio_path = environment.home.join("failed recording.wav");
    pcm_wav(&audio_path);

    let first_started = Instant::now();
    let failed = environment.run(&["retry", audio_path.to_str().unwrap()]);
    let first_elapsed = first_started.elapsed();
    assert!(!failed.status.success(), "{}", output_details(&failed));

    let database = Db::open(&environment.database_path()).unwrap();
    let failed_entry = database
        .get_last_failed_transcription()
        .unwrap()
        .expect("initial failure was not recorded");
    let recording_id = failed_entry.id;
    let initial = database
        .get_recording(recording_id)
        .unwrap()
        .expect("canonical failed recording is missing");
    let captured_at = initial.recording.timestamp;
    assert_eq!(initial.recording.status, RecordingStatus::Failed);
    assert_eq!(initial.attempts.len(), 1);
    assert_eq!(initial.attempts[0].status, "error");
    let first_latency = assert_attempt_latency(&initial.attempts[0], first_delay);
    assert!(first_latency <= first_elapsed.as_millis() as i64);
    drop(database);

    let connection = rusqlite::Connection::open(environment.database_path()).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_retry_transcript
             BEFORE INSERT ON transcripts
             BEGIN
                 SELECT RAISE(ABORT, 'forced retry persistence failure');
             END;",
        )
        .unwrap();
    drop(connection);

    let mut daemon_command = environment.command();
    let mut daemon = DaemonGuard(Some(
        daemon_command
            .arg("daemon")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !environment.socket_path().exists() {
        if daemon.child_mut().try_wait().unwrap().is_some() {
            panic!(
                "daemon exited before opening its socket\n{}",
                output_details(&daemon.0.take().unwrap().wait_with_output().unwrap())
            );
        }
        assert!(Instant::now() < deadline, "daemon did not open its socket");
        thread::sleep(Duration::from_millis(10));
    }

    let backend = Backend::new(false);
    backend.send(Request::Status);
    let initial_status = match next_reply(&backend) {
        Reply::Status(Ok(status)) => status,
        reply => panic!("unexpected initial status reply: {reply:?}"),
    };
    assert_eq!(initial_status.state, DaemonState::Idle);
    assert_eq!(initial_status.last_recording_id, None);
    let initial_generation = initial_status.last_recording_generation;

    backend.send(Request::Retry(recording_id));
    match next_reply(&backend) {
        Reply::Action(Ok(())) => {}
        reply => panic!("unexpected persistence-failure action reply: {reply:?}"),
    }
    let persistence_failure_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            Instant::now() < persistence_failure_deadline,
            "persistence failure did not reach an error state"
        );
        backend.send(Request::Status);
        match next_reply(&backend) {
            Reply::Status(Ok(status)) if status.state == DaemonState::Error => {
                assert_eq!(status.last_recording_id, None);
                assert_eq!(status.last_recording_generation, initial_generation);
                assert!(
                    status
                        .error
                        .starts_with("failed to save recovered transcription:"),
                    "{}",
                    status.error
                );
                break;
            }
            Reply::Status(Ok(_)) => thread::sleep(Duration::from_millis(10)),
            reply => panic!("unexpected persistence-failure status reply: {reply:?}"),
        }
    }
    let status_response = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(Client::new().status())
        .unwrap();
    assert!(status_response.success, "{}", status_response.error);
    assert_eq!(
        status_response.data.get(DATA_KEY_TEXT).map(String::as_str),
        Some("recovered but unsaved")
    );
    let database = Db::open(&environment.database_path()).unwrap();
    let after_persistence_failure = database.get_recording(recording_id).unwrap().unwrap();
    assert_eq!(
        after_persistence_failure.recording.status,
        RecordingStatus::Failed
    );
    assert_eq!(after_persistence_failure.attempts.len(), 1);
    drop(database);
    let connection = rusqlite::Connection::open(environment.database_path()).unwrap();
    connection
        .execute_batch("DROP TRIGGER reject_retry_transcript;")
        .unwrap();
    drop(connection);
    backend.send(Request::Cancel);
    match next_reply(&backend) {
        Reply::Action(Ok(())) => {}
        reply => panic!("unexpected persistence-failure cancel reply: {reply:?}"),
    }

    backend.send(Request::Retry(recording_id));
    match next_reply(&backend) {
        Reply::Action(Ok(())) => {}
        reply => panic!("unexpected failed-retry action reply: {reply:?}"),
    }
    let failed_retry_deadline = Instant::now() + Duration::from_secs(5);
    let failed_retry_status = loop {
        assert!(
            Instant::now() < failed_retry_deadline,
            "failed retry did not reach a terminal state"
        );
        backend.send(Request::Status);
        match next_reply(&backend) {
            Reply::Status(Ok(status))
                if status.last_recording_generation > initial_generation
                    && status.state == DaemonState::Error =>
            {
                break status;
            }
            Reply::Status(Ok(_)) => thread::sleep(Duration::from_millis(10)),
            reply => panic!("unexpected failed-retry status reply: {reply:?}"),
        }
    };
    assert_eq!(failed_retry_status.state, DaemonState::Error);
    assert_eq!(failed_retry_status.last_recording_id, Some(recording_id));

    let database = Db::open(&environment.database_path()).unwrap();
    let after_failed_retry = database.get_recording(recording_id).unwrap().unwrap();
    assert_eq!(after_failed_retry.recording.id, recording_id);
    assert_eq!(after_failed_retry.recording.timestamp, captured_at);
    assert_eq!(after_failed_retry.recording.status, RecordingStatus::Failed);
    assert_eq!(after_failed_retry.attempts.len(), 2);
    assert_attempt_latency(&after_failed_retry.attempts[1], second_delay);
    drop(database);

    backend.send(Request::Cancel);
    match next_reply(&backend) {
        Reply::Action(Ok(())) => {}
        reply => panic!("unexpected cancel reply: {reply:?}"),
    }

    backend.send(Request::Retry(recording_id));
    match next_reply(&backend) {
        Reply::Action(Ok(())) => {}
        reply => panic!("unexpected stalled-retry action reply: {reply:?}"),
    }
    stalled_received_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("stalled retry did not reach the provider");
    backend.send(Request::Cancel);
    match next_reply(&backend) {
        Reply::Action(Ok(())) => {}
        reply => panic!("unexpected stalled-retry cancel reply: {reply:?}"),
    }
    release_stalled_tx.send(()).unwrap();
    let cancellation_deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < cancellation_deadline {
        backend.send(Request::Status);
        match next_reply(&backend) {
            Reply::Status(Ok(status)) => {
                assert_eq!(status.state, DaemonState::Idle);
                assert_eq!(status.last_recording_id, Some(recording_id));
                assert_eq!(
                    status.last_recording_generation,
                    failed_retry_status.last_recording_generation
                );
            }
            reply => panic!("unexpected cancelled-retry status reply: {reply:?}"),
        }
        thread::sleep(Duration::from_millis(20));
    }
    let database = Db::open(&environment.database_path()).unwrap();
    let after_cancel = database.get_recording(recording_id).unwrap().unwrap();
    assert_eq!(after_cancel.recording.status, RecordingStatus::Failed);
    assert_eq!(after_cancel.attempts.len(), 2);
    drop(database);

    backend.send(Request::Retry(recording_id));
    match next_reply(&backend) {
        Reply::Action(Ok(())) => {}
        reply => panic!("unexpected successful-retry action reply: {reply:?}"),
    }
    let recovered_deadline = Instant::now() + Duration::from_secs(5);
    let recovered_status = loop {
        assert!(
            Instant::now() < recovered_deadline,
            "successful retry did not reach a terminal state"
        );
        backend.send(Request::Status);
        match next_reply(&backend) {
            Reply::Status(Ok(status))
                if status.last_recording_generation
                    > failed_retry_status.last_recording_generation
                    && status.state == DaemonState::Idle =>
            {
                break status;
            }
            Reply::Status(Ok(_)) => thread::sleep(Duration::from_millis(10)),
            reply => panic!("unexpected successful-retry status reply: {reply:?}"),
        }
    };
    assert_eq!(recovered_status.state, DaemonState::Idle);
    assert_eq!(recovered_status.last_recording_id, Some(recording_id));

    let database = Db::open(&environment.database_path()).unwrap();
    let recovered = database.get_recording(recording_id).unwrap().unwrap();
    assert_eq!(recovered.recording.id, recording_id);
    assert_eq!(recovered.recording.timestamp, captured_at);
    assert_eq!(recovered.recording.status, RecordingStatus::Complete);
    assert_eq!(recovered.recording.text, "recovered words");
    assert_eq!(recovered.attempts.len(), 3);
    assert_eq!(recovered.attempts[0].attempt_no, 1);
    assert_eq!(recovered.attempts[0].status, "error");
    assert_eq!(recovered.attempts[1].attempt_no, 2);
    assert_eq!(recovered.attempts[1].status, "error");
    assert_eq!(recovered.attempts[2].attempt_no, 3);
    assert_eq!(recovered.attempts[2].status, "complete");
    assert_eq!(recovered.revisions.len(), 1);
    assert_eq!(recovered.revisions[0].revision_no, 0);
    assert_eq!(recovered.revisions[0].text, "recovered words");
    assert_eq!(recovered.revisions[0].source, "model");
    let history = database.get_history(&HistoryQuery::default()).unwrap();
    assert_eq!(
        history.total, 1,
        "retry created a second canonical recording"
    );
    assert_attempt_latency(&recovered.attempts[2], recovered_delay);
    drop(database);

    backend.send(Request::SaveRevision {
        id: recording_id,
        expected_revision: 0,
        text: "edited words".to_string(),
    });
    let edited = match next_reply(&backend) {
        Reply::Saved(Ok(detail)) => detail,
        reply => panic!("unexpected save reply: {reply:?}"),
    };
    assert_eq!(edited.recording.revision, 1);
    assert_eq!(edited.recording.text, "edited words");

    backend.send(Request::SaveRevision {
        id: recording_id,
        expected_revision: 0,
        text: "stale overwrite".to_string(),
    });
    match next_reply(&backend) {
        Reply::Saved(Err(error)) => assert!(error.contains("revision conflict"), "{error}"),
        reply => panic!("stale revision unexpectedly saved: {reply:?}"),
    }

    backend.send(Request::RestoreRevision {
        id: recording_id,
        revision: 0,
        expected_revision: 1,
    });
    let restored = match next_reply(&backend) {
        Reply::Saved(Ok(detail)) => detail,
        reply => panic!("unexpected restore reply: {reply:?}"),
    };
    assert_eq!(restored.recording.revision, 2);
    assert_eq!(restored.recording.text, "recovered words");
    assert_eq!(restored.revisions.len(), 3);
    assert_eq!(restored.revisions[2].source, "restore");

    drop(backend);
    let daemon_output = daemon.stop();
    assert!(
        daemon_output.status.success(),
        "{}",
        output_details(&daemon_output)
    );

    let requests = provider.join().unwrap();
    assert_eq!(requests.len(), 5);
    for request in requests {
        let request = String::from_utf8_lossy(&request);
        assert!(request.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
        assert!(request.contains("authorization: Bearer integration-test-key"));
        assert!(request.contains("failed recording.wav"));
        assert!(request.contains("integration-test-model"));
    }
}
