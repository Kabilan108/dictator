//! One parec capture process feeding a [`Timeline`].
//!
//! A stream is shared through `Arc`: the warm-microphone supervisor and an
//! active recording can both hold it, and the process stops when the last
//! holder drops it. Releasing or replacing the warm stream therefore never
//! cuts off a recording that is still using it.

use std::cell::Cell;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use tracing::{error, info, warn};

use super::devices::{Fingerprint, Resolution, ResolvedMicrophone};
use super::timeline::Timeline;

/// A warm stream without audio for this long is not used for a recording.
pub(crate) const STALE_AFTER: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LevelSample {
    pub rms: f64,
    pub peak: f64,
}

pub type LevelObserver = Arc<dyn Fn(LevelSample) + Send + Sync>;

pub(crate) struct LevelState {
    pub observer: Option<LevelObserver>,
    pub interval: Duration,
    pub last_level_at: Option<Instant>,
}

pub(crate) type Resolver = Arc<dyn Fn() -> Result<Resolution> + Send + Sync>;
pub(crate) type FingerprintFn = Arc<dyn Fn() -> Option<Fingerprint> + Send + Sync>;
pub(crate) type CaptureCommand = Arc<dyn Fn(&str, u32) -> Command + Send + Sync>;
pub(crate) type SubscribeCommand = Arc<dyn Fn() -> Command + Send + Sync>;

/// External processes used for capture. Tests replace them with fakes.
#[derive(Clone)]
pub(crate) struct CaptureBackend {
    pub resolve: Resolver,
    pub fingerprint: FingerprintFn,
    pub capture: CaptureCommand,
    pub subscribe: Option<SubscribeCommand>,
}

impl CaptureBackend {
    pub fn system() -> Self {
        Self {
            resolve: Arc::new(super::devices::resolve_microphone),
            fingerprint: Arc::new(super::devices::preferences_fingerprint),
            capture: Arc::new(parec_command),
            subscribe: Some(Arc::new(|| {
                let mut command = Command::new("pactl");
                command.arg("subscribe");
                command
            })),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct StreamOptions {
    pub sample_rate: u32,
    pub frames_per_block: usize,
    pub ring_capacity: usize,
}

thread_local! {
    static CAPTURE_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// True on a capture owner thread, where level observers run.
pub(crate) fn on_capture_thread() -> bool {
    CAPTURE_THREAD.with(Cell::get)
}

pub(crate) struct StreamShared {
    pub timeline: Mutex<Timeline>,
    /// Signalled after each chunk and when the stream ends.
    pub progress: Condvar,
}

pub(crate) struct CaptureStream {
    microphone: ResolvedMicrophone,
    shared: Arc<StreamShared>,
    opened_at: Instant,
    owner: Mutex<Option<OwnerHandle>>,
}

struct OwnerHandle {
    command_tx: mpsc::SyncSender<StreamCommand>,
    thread: JoinHandle<()>,
}

enum StreamCommand {
    LevelReady,
    CaptureEnded,
    Stop,
}

#[derive(Default)]
struct LevelMailbox {
    latest: Mutex<LevelSample>,
    notification_pending: AtomicBool,
}

impl CaptureStream {
    /// Starts capture from `microphone`. `on_end` runs on the owner thread if
    /// the process ends on its own rather than through [`CaptureStream::close`].
    pub fn open(
        command: &CaptureCommand,
        microphone: ResolvedMicrophone,
        options: StreamOptions,
        level: Arc<Mutex<LevelState>>,
        on_end: Option<Box<dyn FnOnce() + Send>>,
    ) -> Result<Arc<Self>> {
        let shared = Arc::new(StreamShared {
            timeline: Mutex::new(Timeline::new(options.sample_rate, options.ring_capacity)),
            progress: Condvar::new(),
        });
        let mut process = command(&microphone.source_name, options.sample_rate);
        process
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let owner_shared = Arc::clone(&shared);
        let level_mailbox = Arc::new(LevelMailbox::default());
        // One coalesced level notification, capture completion, and control.
        let (command_tx, command_rx) = mpsc::sync_channel::<StreamCommand>(3);
        let reader_command_tx = command_tx.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let source_name = microphone.source_name.clone();
        let microphone_name = microphone.name.clone();
        let frames_per_block = options.frames_per_block;

        let thread = std::thread::Builder::new()
            .name("dictator-audio".into())
            .spawn(move || {
                CAPTURE_THREAD.with(|flag| flag.set(true));
                let mut child = match process.spawn() {
                    Ok(child) => child,
                    Err(err) => {
                        let _ = ready_tx.send(Err(anyhow!(
                            "failed to open audio stream with parec: {err}"
                        )));
                        return;
                    }
                };
                let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take())
                else {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = ready_tx.send(Err(anyhow!(
                        "failed to open audio stream: parec pipes were unavailable"
                    )));
                    return;
                };
                let stderr_reader = match std::thread::Builder::new()
                    .name("dictator-audio-stderr".into())
                    .spawn(move || read_stderr_bounded(stderr))
                {
                    Ok(reader) => reader,
                    Err(err) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = ready_tx
                            .send(Err(anyhow!("failed to start audio error reader: {err}")));
                        return;
                    }
                };
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let detail = join_stderr_reader(stderr_reader);
                        let _ = ready_tx.send(Err(anyhow!(
                            "failed to open audio stream: parec exited with {status}{detail}"
                        )));
                        return;
                    }
                    Ok(None) => {}
                    Err(err) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = stderr_reader.join();
                        let _ = ready_tx
                            .send(Err(anyhow!("failed to inspect parec audio stream: {err}")));
                        return;
                    }
                }

                let reader_shared = Arc::clone(&owner_shared);
                let reader_level = Arc::clone(&level);
                let reader_mailbox = Arc::clone(&level_mailbox);
                let reader = match std::thread::Builder::new()
                    .name("dictator-audio-reader".into())
                    .spawn(move || {
                        read_parec_stream(
                            stdout,
                            frames_per_block,
                            &reader_shared,
                            &reader_level,
                            &reader_mailbox,
                            &reader_command_tx,
                        );
                        let _ = reader_command_tx.send(StreamCommand::CaptureEnded);
                    }) {
                    Ok(reader) => reader,
                    Err(err) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        let detail = join_stderr_reader(stderr_reader);
                        let _ = ready_tx.send(Err(anyhow!(
                            "failed to start audio stream reader: {err}{detail}"
                        )));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));

                let mut stopped = false;
                while let Ok(command) = command_rx.recv() {
                    match command {
                        StreamCommand::LevelReady => {
                            level_mailbox
                                .notification_pending
                                .store(false, Ordering::Release);
                            let sample = *level_mailbox.latest.lock().unwrap();
                            deliver_level_sample(&level, sample);
                        }
                        StreamCommand::CaptureEnded => break,
                        StreamCommand::Stop => {
                            stopped = true;
                            break;
                        }
                    }
                }
                if let Err(err) = child.kill()
                    && err.kind() != std::io::ErrorKind::InvalidInput
                {
                    warn!(err = %err, "failed to stop parec capture");
                }
                let status = child.wait();
                if reader.join().is_err() {
                    error!("audio stream reader panicked");
                }
                let detail = join_stderr_reader(stderr_reader);
                let message = (!stopped).then(|| match status {
                    Ok(status) => {
                        format!("audio input stopped: parec exited with {status}{detail}")
                    }
                    Err(err) => format!("audio input stopped: failed to wait for parec: {err}"),
                });
                if let Some(message) = &message {
                    warn!(source = source_name, err = %message, "audio stream ended");
                }
                owner_shared
                    .timeline
                    .lock()
                    .unwrap()
                    .mark_ended(message.clone());
                owner_shared.progress.notify_all();
                if message.is_some()
                    && let Some(on_end) = on_end
                {
                    on_end();
                }
            })
            .map_err(|e| anyhow!("failed to spawn audio thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => {
                info!(
                    microphone_id = microphone.id,
                    microphone = microphone_name,
                    source = microphone.source_name,
                    "audio input opened"
                );
                Ok(Arc::new(Self {
                    microphone,
                    shared,
                    opened_at: Instant::now(),
                    owner: Mutex::new(Some(OwnerHandle { command_tx, thread })),
                }))
            }
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                let _ = thread.join();
                Err(anyhow!("failed to open audio stream: audio thread exited"))
            }
        }
    }

    pub fn microphone(&self) -> &ResolvedMicrophone {
        &self.microphone
    }

    pub fn shared(&self) -> &Arc<StreamShared> {
        &self.shared
    }

    /// Delivering audio now: running and a chunk arrived within [`STALE_AFTER`].
    pub fn is_live(&self, now: Instant) -> bool {
        let timeline = self.shared.timeline.lock().unwrap();
        !timeline.is_ended()
            && timeline
                .last_chunk_at()
                .is_some_and(|at| now.saturating_duration_since(at) < STALE_AFTER)
    }

    /// Ended, or silent for `grace` since it opened or last delivered audio.
    pub fn is_stalled(&self, now: Instant, grace: Duration) -> bool {
        let timeline = self.shared.timeline.lock().unwrap();
        let last = timeline.last_chunk_at().unwrap_or(self.opened_at);
        timeline.is_ended() || now.saturating_duration_since(last) >= grace
    }

    /// Stops the process and joins its threads. Safe to call repeatedly.
    pub fn close(&self) {
        let owner = match self.owner.lock() {
            Ok(mut owner) => owner.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(owner) = owner else {
            return;
        };
        let _ = owner.command_tx.send(StreamCommand::Stop);
        if owner.thread.thread().id() == std::thread::current().id() {
            // A level observer dropped the last holder; the owner exits after
            // the observer returns.
            return;
        }
        if owner.thread.join().is_err() {
            error!("audio stream thread panicked");
        }
    }
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        self.close();
    }
}

/// Requests a small server-side buffer so stop and pre-roll timing stay
/// within tens of milliseconds. PulseAudio's default record latency is about
/// one second. `node.dont-reconnect` makes PipeWire end the stream when its
/// device disappears instead of moving it to another source, which could be
/// a Bluetooth headset.
pub(crate) fn parec_command(source_name: &str, sample_rate: u32) -> Command {
    let mut command = Command::new("parec");
    command.args([
        "--record",
        "--raw",
        "--format=float32le",
        "--channels=1",
        "--client-name=dictator",
        "--stream-name=Dictation",
        "--latency-msec=50",
        "--process-time-msec=10",
        "--property=node.dont-reconnect=true",
        "--device",
        source_name,
        "--rate",
        &sample_rate.to_string(),
    ]);
    command
}

fn read_parec_stream(
    mut stdout: impl Read,
    frames_per_block: usize,
    shared: &StreamShared,
    level: &Mutex<LevelState>,
    mailbox: &LevelMailbox,
    command_tx: &mpsc::SyncSender<StreamCommand>,
) {
    let mut bytes = vec![0; frames_per_block.max(1).saturating_mul(4)];
    let mut remainder = Vec::with_capacity(3);
    loop {
        let read = match stdout.read(&mut bytes) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                warn!(err = %err, "failed to read parec audio data");
                break;
            }
        };
        let samples = decode_float32le(&bytes[..read], &mut remainder);
        if samples.is_empty() {
            continue;
        }
        let recording = {
            let mut timeline = shared.timeline.lock().unwrap();
            timeline.push(&samples, Instant::now());
            timeline.has_session()
        };
        shared.progress.notify_all();
        if recording {
            queue_level_sample(level, mailbox, command_tx, &samples);
        }
    }
}

fn decode_float32le(bytes: &[u8], remainder: &mut Vec<u8>) -> Vec<f32> {
    let mut combined = Vec::with_capacity(remainder.len() + bytes.len());
    combined.append(remainder);
    combined.extend_from_slice(bytes);
    let complete = combined.len() / 4 * 4;
    let samples = combined[..complete]
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect();
    remainder.extend_from_slice(&combined[complete..]);
    samples
}

fn read_stderr_bounded(mut stderr: impl Read) -> String {
    const LIMIT: usize = 16 * 1024;
    let mut stored = Vec::new();
    let mut buffer = [0; 1024];
    let mut truncated = false;
    loop {
        let read = match stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let keep = read.min(LIMIT.saturating_sub(stored.len()));
        stored.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    let detail = String::from_utf8_lossy(&stored);
    let detail = detail.trim();
    if detail.is_empty() {
        String::new()
    } else if truncated {
        format!(": {detail} [stderr truncated]")
    } else {
        format!(": {detail}")
    }
}

fn join_stderr_reader(reader: JoinHandle<String>) -> String {
    reader
        .join()
        .unwrap_or_else(|_| ": failed to read parec stderr".into())
}

fn queue_level_sample(
    level: &Mutex<LevelState>,
    mailbox: &LevelMailbox,
    command_tx: &mpsc::SyncSender<StreamCommand>,
    samples: &[f32],
) {
    {
        let Ok(mut guard) = level.try_lock() else {
            return;
        };
        if guard.observer.is_none() {
            return;
        }
        let now = Instant::now();
        if guard.interval > Duration::ZERO
            && let Some(last) = guard.last_level_at
            && now.duration_since(last) < guard.interval
        {
            return;
        }
        guard.last_level_at = Some(now);
    }

    let Ok(mut latest) = mailbox.latest.try_lock() else {
        return;
    };
    *latest = calculate_level(samples);
    drop(latest);
    if !mailbox.notification_pending.swap(true, Ordering::AcqRel)
        && command_tx.try_send(StreamCommand::LevelReady).is_err()
    {
        mailbox.notification_pending.store(false, Ordering::Release);
    }
}

pub(crate) fn deliver_level_sample(level: &Mutex<LevelState>, sample: LevelSample) {
    let observer = level.lock().unwrap().observer.clone();
    let Some(observer) = observer else {
        return;
    };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| observer(sample))).is_err() {
        warn!("audio level observer panicked");
    }
}

pub fn calculate_level(samples: &[f32]) -> LevelSample {
    if samples.is_empty() {
        return LevelSample::default();
    }
    let mut sum_squares = 0.0f64;
    let mut peak = 0.0f64;
    for sample in samples {
        let value = (*sample as f64).abs();
        sum_squares += value * value;
        if value > peak {
            peak = value;
        }
    }
    LevelSample {
        rms: (sum_squares / samples.len() as f64).sqrt(),
        peak: peak.min(1.0),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn quiet_level() -> Arc<Mutex<LevelState>> {
        Arc::new(Mutex::new(LevelState {
            observer: None,
            interval: Duration::ZERO,
            last_level_at: None,
        }))
    }

    pub(crate) fn microphone(source_name: &str) -> ResolvedMicrophone {
        ResolvedMicrophone {
            id: format!("source:{source_name}"),
            name: source_name.to_owned(),
            source_name: source_name.to_owned(),
        }
    }

    pub(crate) fn options() -> StreamOptions {
        StreamOptions {
            sample_rate: 16_000,
            frames_per_block: 1024,
            ring_capacity: 160_000,
        }
    }

    /// A fake parec that writes `script`'s output as raw float32 audio.
    pub(crate) fn shell_capture(script: &'static str) -> CaptureCommand {
        Arc::new(move |_, _| {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            command
        })
    }

    /// 100 ms of float32 silence every 100 ms, until killed.
    pub(crate) const LIVE_SILENCE: &str =
        "while :; do head -c 6400 /dev/zero || exit 1; sleep 0.1; done";

    pub(crate) fn wait_for(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "condition not reached in 5 s");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn level_calculation() {
        let level = calculate_level(&[0.5, -0.5, 0.5, -0.5]);
        assert!((level.rms - 0.5).abs() < 1e-9);
        assert!((level.peak - 0.5).abs() < 1e-9);
        assert_eq!(calculate_level(&[]), LevelSample::default());
        assert_eq!(calculate_level(&[2.0]).peak, 1.0);
    }

    #[test]
    fn parec_float_decoder_preserves_partial_samples_between_reads() {
        let expected = [0.25f32, -0.75f32];
        let bytes: Vec<_> = expected
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let mut remainder = Vec::new();

        assert!(decode_float32le(&bytes[..3], &mut remainder).is_empty());
        assert_eq!(remainder, bytes[..3]);
        assert_eq!(decode_float32le(&bytes[3..5], &mut remainder), [0.25]);
        assert_eq!(remainder, bytes[4..5]);
        assert_eq!(decode_float32le(&bytes[5..], &mut remainder), [-0.75]);
        assert!(remainder.is_empty());
    }

    #[test]
    fn parec_command_captures_the_resolved_source_without_reconnecting() {
        let command = parec_command("alsa_input.usb-mic", 16_000);
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(command.get_program(), "parec");
        assert_eq!(
            args,
            [
                "--record",
                "--raw",
                "--format=float32le",
                "--channels=1",
                "--client-name=dictator",
                "--stream-name=Dictation",
                "--latency-msec=50",
                "--process-time-msec=10",
                "--property=node.dont-reconnect=true",
                "--device",
                "alsa_input.usb-mic",
                "--rate",
                "16000",
            ]
        );
    }

    #[test]
    fn parec_stderr_is_drained_but_error_text_is_bounded() {
        let detail = read_stderr_bounded(std::io::Cursor::new(vec![b'x'; 32 * 1024]));
        assert!(detail.ends_with(" [stderr truncated]"));
        assert!(detail.len() < 17 * 1024);
    }

    #[test]
    fn level_mailbox_coalesces_to_latest_sample() {
        let level = Mutex::new(LevelState {
            observer: Some(Arc::new(|_| {})),
            interval: Duration::ZERO,
            last_level_at: None,
        });
        let mailbox = LevelMailbox::default();
        let (command_tx, command_rx) = mpsc::sync_channel(2);

        queue_level_sample(&level, &mailbox, &command_tx, &[0.1]);
        queue_level_sample(&level, &mailbox, &command_tx, &[0.5]);
        queue_level_sample(&level, &mailbox, &command_tx, &[0.9]);

        assert!(matches!(
            command_rx.try_recv(),
            Ok(StreamCommand::LevelReady)
        ));
        assert!(command_rx.try_recv().is_err());
        let latest = *mailbox.latest.lock().unwrap();
        assert!((latest.peak - 0.9).abs() < 1e-6);
    }

    #[test]
    fn stream_reports_an_unexpected_exit_and_runs_its_end_hook() {
        let (ended_tx, ended_rx) = mpsc::channel();
        let stream = CaptureStream::open(
            &shell_capture("sleep 0.2; head -c 3200 /dev/zero; echo 'device gone' >&2; exit 3"),
            microphone("fake"),
            options(),
            quiet_level(),
            Some(Box::new(move || ended_tx.send(()).unwrap())),
        )
        .unwrap();

        ended_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let timeline = stream.shared().timeline.lock().unwrap();
        assert!(timeline.is_ended());
        let error = timeline.error().unwrap();
        assert!(error.contains("device gone"), "{error}");
        drop(timeline);
        assert!(!stream.is_live(Instant::now()));
        assert!(stream.is_stalled(Instant::now(), Duration::from_secs(60)));
    }

    #[test]
    fn closing_a_live_stream_stops_the_process_without_an_error() {
        let (ended_tx, ended_rx) = mpsc::channel::<()>();
        let stream = CaptureStream::open(
            &shell_capture(LIVE_SILENCE),
            microphone("fake"),
            options(),
            quiet_level(),
            Some(Box::new(move || ended_tx.send(()).unwrap())),
        )
        .unwrap();
        wait_for(|| stream.is_live(Instant::now()));

        stream.close();

        let timeline = stream.shared().timeline.lock().unwrap();
        assert!(timeline.is_ended());
        assert!(timeline.error().is_none());
        assert!(ended_rx.try_recv().is_err());
    }

    #[test]
    fn missing_capture_program_fails_to_open() {
        let command: CaptureCommand =
            Arc::new(|_, _| Command::new("/nonexistent/dictator-test-parec"));
        let error =
            CaptureStream::open(&command, microphone("fake"), options(), quiet_level(), None)
                .err()
                .unwrap()
                .to_string();
        assert!(error.contains("failed to open audio stream"), "{error}");
    }
}
