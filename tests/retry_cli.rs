use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::{Duration, Instant};

use dictator::storage::Db;

struct TestEnvironment {
    _root: tempfile::TempDir,
    home: PathBuf,
    config_home: PathBuf,
    data_home: PathBuf,
    state_home: PathBuf,
    runtime_dir: PathBuf,
}

impl TestEnvironment {
    fn new(endpoint: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let config_home = root.path().join("config");
        let data_home = root.path().join("data");
        let state_home = root.path().join("state");
        let runtime_dir = root.path().join("runtime");
        for path in [&home, &config_home, &data_home, &state_home, &runtime_dir] {
            std::fs::create_dir(path).unwrap();
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
                        "key": "test-key",
                        "model": "test-model"
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
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dictator"));
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_STATE_HOME", &self.state_home)
            .env("XDG_RUNTIME_DIR", &self.runtime_dir);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn database_path(&self) -> PathBuf {
        self.data_home.join("dictator").join("app.db")
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

fn spawn_provider(
    responses: Vec<(&'static str, &'static str)>,
) -> (String, thread::JoinHandle<Vec<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "provider was not contacted");
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => panic!("provider accept failed: {err}"),
                }
            };
            requests.push(read_request(&mut stream));
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
        requests
    });
    (endpoint, server)
}

fn spawn_stalled_provider() -> (String, std_mpsc::Receiver<()>, thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (received_tx, received_rx) = std_mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "provider was not contacted");
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("provider accept failed: {err}"),
            }
        };
        let request = read_request(&mut stream);
        received_tx.send(()).unwrap();

        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(
            stream.read(&mut byte).unwrap(),
            0,
            "retry sent unexpected data after its HTTP request"
        );
        request
    });
    (endpoint, received_rx, server)
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
        assert!(
            request.len() < 2 * 1024 * 1024,
            "provider request too large"
        );
    }
}

fn output_details(output: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn wait_for_output(mut child: Child, timeout: Duration) -> Result<Output, Output> {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().unwrap().is_some() {
            return Ok(child.wait_with_output().unwrap());
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            return Err(child.wait_with_output().unwrap());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn explicit_failure_is_pending_and_no_argument_retry_recovers_it() {
    let (endpoint, server) = spawn_provider(vec![
        ("503 Service Unavailable", r#"{"error":"try later"}"#),
        ("200 OK", r#"{"text":"recovered words"}"#),
    ]);
    let environment = TestEnvironment::new(&endpoint);
    let audio_path = environment.home.join("failed recording.wav");
    pcm_wav(&audio_path);

    let failed = environment.run(&["retry", audio_path.to_str().unwrap()]);
    assert!(!failed.status.success(), "{}", output_details(&failed));
    assert!(failed.stdout.is_empty(), "{}", output_details(&failed));
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("retry failed:"),
        "{}",
        output_details(&failed)
    );
    assert!(audio_path.exists(), "failed retry removed the recording");

    let database_path = environment.database_path();
    let database = Db::open(&database_path).unwrap();
    let pending = database
        .get_last_failed_transcription()
        .unwrap()
        .expect("failed retry was not recorded");
    assert_eq!(pending.duration_ms, 1_000);
    assert_eq!(
        Path::new(&pending.audio_path),
        audio_path.canonicalize().unwrap()
    );
    drop(database);

    let recovered = environment.run(&["retry"]);
    assert!(recovered.status.success(), "{}", output_details(&recovered));
    assert_eq!(recovered.stdout, b"recovered words\n");
    assert!(
        audio_path.exists(),
        "successful retry removed the recording"
    );

    let database = Db::open(&database_path).unwrap();
    assert!(database.get_last_failed_transcription().unwrap().is_none());
    let transcripts = database.get_transcripts(-1).unwrap();
    assert_eq!(transcripts.len(), 1);
    assert_eq!(transcripts[0].text, "recovered words");
    assert_eq!(transcripts[0].duration_ms, 1_000);
    assert_eq!(transcripts[0].model, "test-model");
    assert_eq!(
        Path::new(&transcripts[0].audio_path),
        audio_path.canonicalize().unwrap()
    );

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests {
        let request = String::from_utf8_lossy(&request);
        assert!(request.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
        assert!(request.contains("authorization: Bearer test-key"));
        assert!(request.contains("failed recording.wav"));
        assert!(request.contains("test-model"));
    }
}

#[test]
fn retry_without_a_pending_failure_is_helpful_and_nonzero() {
    let environment = TestEnvironment::new("http://127.0.0.1:9");

    let output = environment.run(&["retry"]);

    assert!(!output.status.success(), "{}", output_details(&output));
    assert!(output.stdout.is_empty(), "{}", output_details(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("no failed transcription is available to retry"),
        "{}",
        output_details(&output)
    );
}

#[test]
fn no_argument_retry_clears_a_failure_stored_through_a_symlink() {
    let (endpoint, server) = spawn_provider(vec![("200 OK", r#"{"text":"symlink recovered"}"#)]);
    let environment = TestEnvironment::new(&endpoint);
    let audio_path = environment.home.join("recording.wav");
    let symlink_path = environment.home.join("pending.wav");
    pcm_wav(&audio_path);
    std::os::unix::fs::symlink(&audio_path, &symlink_path).unwrap();

    let database_path = environment.database_path();
    std::fs::create_dir_all(database_path.parent().unwrap()).unwrap();
    let database = Db::open(&database_path).unwrap();
    database
        .save_failed_transcription(1_000, symlink_path.to_str().unwrap())
        .unwrap();
    drop(database);

    let output = environment.run(&["retry"]);

    assert!(output.status.success(), "{}", output_details(&output));
    assert_eq!(output.stdout, b"symlink recovered\n");
    assert!(audio_path.exists());
    assert!(symlink_path.exists());
    let database = Db::open(&database_path).unwrap();
    assert!(
        database.get_last_failed_transcription().unwrap().is_none(),
        "successful retry left the symlink path pending"
    );
    server.join().unwrap();
}

#[test]
fn provider_success_is_printed_when_transcript_persistence_fails() {
    let (endpoint, server) =
        spawn_provider(vec![("200 OK", r#"{"text":"provider already succeeded"}"#)]);
    let environment = TestEnvironment::new(&endpoint);
    let audio_path = environment.home.join("recording.wav");
    pcm_wav(&audio_path);

    let database_path = environment.database_path();
    std::fs::create_dir_all(database_path.parent().unwrap()).unwrap();
    let database = Db::open(&database_path).unwrap();
    database
        .save_failed_transcription(1_000, audio_path.to_str().unwrap())
        .unwrap();
    drop(database);
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_retry_transcript
             BEFORE INSERT ON transcripts
             BEGIN
                 SELECT RAISE(ABORT, 'forced transcript failure');
             END;",
        )
        .unwrap();
    drop(connection);

    let output = environment.run(&["retry"]);

    assert!(!output.status.success(), "{}", output_details(&output));
    assert_eq!(output.stdout, b"provider already succeeded\n");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("failed to save"),
        "{}",
        output_details(&output)
    );
    assert!(audio_path.exists());
    let database = Db::open(&database_path).unwrap();
    assert!(database.get_transcripts(-1).unwrap().is_empty());
    assert!(database.get_last_failed_transcription().unwrap().is_some());
    server.join().unwrap();
}

#[test]
fn invalid_configuration_does_not_mark_explicit_audio_as_failed() {
    let environment = TestEnvironment::new("http://127.0.0.1:9");
    std::fs::remove_file(environment.config_home.join("dictator/config.json")).unwrap();
    let audio_path = environment.home.join("recording.wav");
    pcm_wav(&audio_path);

    let output = environment.run(&["retry", audio_path.to_str().unwrap()]);

    assert!(!output.status.success(), "{}", output_details(&output));
    assert!(output.stdout.is_empty(), "{}", output_details(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("API key is required"),
        "{}",
        output_details(&output)
    );
    assert!(audio_path.exists());
    let database = Db::open(&environment.database_path()).unwrap();
    assert!(database.get_last_failed_transcription().unwrap().is_none());
}

#[test]
fn sigint_cancels_a_stalled_retry_without_consuming_the_pending_audio() {
    let (endpoint, request_received, server) = spawn_stalled_provider();
    let environment = TestEnvironment::new(&endpoint);
    let audio_path = environment.home.join("pending.wav");
    pcm_wav(&audio_path);

    let database_path = environment.database_path();
    std::fs::create_dir_all(database_path.parent().unwrap()).unwrap();
    let database = Db::open(&database_path).unwrap();
    database
        .save_failed_transcription(1_000, audio_path.to_str().unwrap())
        .unwrap();
    drop(database);

    let mut first_command = environment.command();
    let mut first = first_command
        .arg("retry")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if request_received
        .recv_timeout(Duration::from_secs(3))
        .is_err()
    {
        first.kill().unwrap();
        let output = first.wait_with_output().unwrap();
        let _ = server.join();
        panic!(
            "first retry did not reach the provider\n{}",
            output_details(&output)
        );
    }

    let mut second_command = environment.command();
    let second = wait_for_output(
        second_command
            .arg("retry")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
        Duration::from_secs(1),
    );

    // SAFETY: `first.id()` is the live child process created above.
    let signal_result = unsafe { libc::kill(first.id() as libc::pid_t, libc::SIGINT) };
    let first = wait_for_output(first, Duration::from_secs(2));
    let request = server.join().unwrap();

    assert_eq!(signal_result, 0, "failed to signal stalled retry");
    let second = second.unwrap_or_else(|output| {
        panic!(
            "concurrent retry did not reject promptly\n{}",
            output_details(&output)
        )
    });
    assert!(!second.status.success(), "{}", output_details(&second));
    assert!(second.stdout.is_empty(), "{}", output_details(&second));
    assert!(
        String::from_utf8_lossy(&second.stderr)
            .contains("another transcription retry is already running"),
        "{}",
        output_details(&second)
    );

    let first = first.unwrap_or_else(|output| {
        panic!(
            "stalled retry did not exit promptly after SIGINT\n{}",
            output_details(&output)
        )
    });
    assert!(!first.status.success(), "{}", output_details(&first));
    assert!(first.stdout.is_empty(), "{}", output_details(&first));
    assert!(
        String::from_utf8_lossy(&first.stderr).contains("transcription cancelled"),
        "{}",
        output_details(&first)
    );
    assert!(
        String::from_utf8_lossy(&request).contains("pending.wav"),
        "provider did not receive the pending recording"
    );
    assert!(audio_path.exists(), "cancelled retry removed its audio");
    let database = Db::open(&database_path).unwrap();
    let pending = database
        .get_last_failed_transcription()
        .unwrap()
        .expect("cancelled retry consumed its pending row");
    assert_eq!(Path::new(&pending.audio_path), audio_path);
}

#[test]
fn retry_help_is_only_written_to_stdout() {
    let environment = TestEnvironment::new("http://127.0.0.1:9");

    let output = environment.run(&["retry", "--help"]);

    assert!(output.status.success(), "{}", output_details(&output));
    assert!(output.stderr.is_empty(), "{}", output_details(&output));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Usage: dictator retry [OPTIONS] [AUDIO_FILE]"));
    assert!(stdout.contains("WAV recording to transcribe"));
    assert!(
        !environment.state_home.join("dictator/app.log").exists(),
        "help initialized application logging"
    );
}
