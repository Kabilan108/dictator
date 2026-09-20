use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use std::{fs::OpenOptions, io::Read, io::Write};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Local};
use tracing::{debug, error, info, warn};

use super::devices::{ResolvedMicrophone, resolve_microphone};
use crate::utils::{AudioConfig, get_path_to_recording, validate_audio_config};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderState {
    Idle,
    Recording,
    Stopped,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LevelSample {
    pub rms: f64,
    pub peak: f64,
}

pub type LevelObserver = Arc<dyn Fn(LevelSample) + Send + Sync>;

struct LevelState {
    observer: Option<LevelObserver>,
    interval: Duration,
    last_level_at: Option<Instant>,
}

struct Inner {
    state: RecorderState,
    start_instant: Instant,
    start_time: DateTime<Local>,
    /// Thread that owns the capture process; sending `StreamCommand::Stop`
    /// stops and closes it.
    stream: Option<StreamHandle>,
}

/// State shared with the capture reader. It deliberately does not own the
/// stream handle, so dropping a live recorder cannot create an ownership cycle.
struct CaptureState {
    buffer: Vec<f32>,
    accepting: bool,
    max_samples: usize,
}

struct StreamHandle {
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

struct OwnerThreadRegistration {
    owner_thread: Arc<Mutex<Option<std::thread::ThreadId>>>,
}

impl OwnerThreadRegistration {
    fn new(owner_thread: Arc<Mutex<Option<std::thread::ThreadId>>>) -> Self {
        *owner_thread.lock().unwrap() = Some(std::thread::current().id());
        Self { owner_thread }
    }
}

impl Drop for OwnerThreadRegistration {
    fn drop(&mut self) {
        let mut owner = self.owner_thread.lock().unwrap();
        if owner.is_some_and(|id| id == std::thread::current().id()) {
            *owner = None;
        }
    }
}

/// Records mono float32 audio from the highest-priority connected input into
/// memory and encodes it as 16-bit PCM WAV on stop.
pub struct Recorder {
    config: AudioConfig,
    control: Mutex<()>,
    inner: Mutex<Inner>,
    capture: Arc<Mutex<CaptureState>>,
    level: Arc<Mutex<LevelState>>,
    stream_error: Arc<Mutex<Option<String>>>,
    owner_thread: Arc<Mutex<Option<std::thread::ThreadId>>>,
}

impl Recorder {
    pub fn new(config: AudioConfig) -> Result<Self> {
        validate_audio_config(&config)?;
        let max_samples = max_samples(&config)?;

        debug!(
            sr = config.sample_rate,
            channels = config.channels,
            bit_depth = config.bit_depth,
            "recorder initialized"
        );

        Ok(Self::from_config(config, max_samples))
    }

    fn from_config(config: AudioConfig, max_samples: usize) -> Self {
        Self {
            config,
            control: Mutex::new(()),
            inner: Mutex::new(Inner {
                state: RecorderState::Idle,
                start_instant: Instant::now(),
                start_time: Local::now(),
                stream: None,
            }),
            capture: Arc::new(Mutex::new(CaptureState {
                buffer: Vec::new(),
                accepting: false,
                max_samples,
            })),
            level: Arc::new(Mutex::new(LevelState {
                observer: None,
                interval: Duration::ZERO,
                last_level_at: None,
            })),
            stream_error: Arc::new(Mutex::new(None)),
            owner_thread: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(config: AudioConfig) -> Self {
        validate_audio_config(&config).expect("valid test audio config");
        let max_samples = max_samples(&config).expect("test capture size fits usize");
        Self::from_config(config, max_samples)
    }

    pub fn set_level_observer(&self, observer: Option<LevelObserver>, interval: Duration) {
        let mut level = self.level.lock().unwrap();
        level.observer = observer;
        level.interval = interval;
        level.last_level_at = None;
    }

    pub fn get_state(&self) -> RecorderState {
        self.inner.lock().unwrap().state
    }

    pub fn is_recording(&self) -> bool {
        self.get_state() == RecorderState::Recording
    }

    pub fn start(&self) -> Result<()> {
        self.reject_owner_thread_control()?;
        let _control = self.control.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();

        if inner.state == RecorderState::Recording {
            bail!("recorder is already recording");
        }

        debug!("starting audio recording");

        inner.start_instant = Instant::now();
        inner.start_time = Local::now();
        self.level.lock().unwrap().last_level_at = None;
        *self.stream_error.lock().unwrap() = None;

        // Reserve the full bounded recording outside the real-time callback.
        // This also makes an impossible allocation fail before opening a device.
        let max_samples = self.capture.lock().unwrap().max_samples;
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(max_samples)
            .map_err(|e| anyhow!("failed to reserve audio capture buffer: {e}"))?;
        {
            let mut capture = self.capture.lock().unwrap();
            capture.buffer = buffer;
            capture.accepting = true;
        }

        let stream = match self.open_stream() {
            Ok(stream) => stream,
            Err(err) => {
                let mut capture = self.capture.lock().unwrap();
                capture.accepting = false;
                capture.buffer = Vec::new();
                error!(err = %err, "failed to open audio stream");
                return Err(err);
            }
        };

        inner.stream = Some(stream);
        inner.state = RecorderState::Recording;
        drop(inner);

        info!(
            max_duration_min = self.config.max_duration_min,
            "recording started"
        );
        Ok(())
    }

    /// Resolves the preferred input once, then opens that exact source on a
    /// dedicated thread. An active capture never follows later preference or
    /// system-default changes.
    fn open_stream(&self) -> Result<StreamHandle> {
        let microphone = resolve_microphone()?;
        self.try_open_stream(microphone)
    }

    fn try_open_stream(&self, microphone: ResolvedMicrophone) -> Result<StreamHandle> {
        let sample_rate = self.config.sample_rate.to_string();
        let frames_per_block = usize::try_from(self.config.frames_per_block)
            .map_err(|_| anyhow!("audio frames per block does not fit usize"))?;
        let capture = Arc::clone(&self.capture);
        let level = Arc::clone(&self.level);
        let stream_error = Arc::clone(&self.stream_error);
        let owner_thread = Arc::clone(&self.owner_thread);
        let level_mailbox = Arc::new(LevelMailbox::default());
        // One coalesced level notification, capture completion, and control.
        let (command_tx, command_rx) = mpsc::sync_channel::<StreamCommand>(3);
        let callback_command_tx = command_tx.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let source_name = microphone.source_name.clone();
        let microphone_name = microphone.name.clone();
        let microphone_id = microphone.id.clone();

        let thread = std::thread::Builder::new()
            .name("dictator-audio".into())
            .spawn(move || {
                let _owner_registration = OwnerThreadRegistration::new(owner_thread);
                let mut child = match parec_command(&source_name, &sample_rate).spawn() {
                    Ok(child) => child,
                    Err(err) => {
                        let _ = ready_tx.send(Err(anyhow!(
                            "failed to open audio stream with parec: {err}"
                        )));
                        return;
                    }
                };
                let Some(stdout) = child.stdout.take() else {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = ready_tx.send(Err(anyhow!(
                        "failed to open audio stream: parec stdout was unavailable"
                    )));
                    return;
                };
                let Some(stderr) = child.stderr.take() else {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = ready_tx.send(Err(anyhow!(
                        "failed to open audio stream: parec stderr was unavailable"
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

                let reader_capture = Arc::clone(&capture);
                let reader_level = Arc::clone(&level);
                let reader_mailbox = Arc::clone(&level_mailbox);
                let reader = match std::thread::Builder::new()
                    .name("dictator-audio-reader".into())
                    .spawn(move || {
                        read_parec_stream(
                            stdout,
                            frames_per_block,
                            &reader_capture,
                            &reader_level,
                            &reader_mailbox,
                            &callback_command_tx,
                        );
                        let _ = callback_command_tx.send(StreamCommand::CaptureEnded);
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
                info!(
                    microphone_id,
                    microphone = microphone_name,
                    source = source_name,
                    "audio input selected"
                );

                let mut stopped = false;
                let mut capture_ended = false;
                while let Ok(command) = command_rx.recv() {
                    match command {
                        StreamCommand::LevelReady => {
                            level_mailbox
                                .notification_pending
                                .store(false, Ordering::Release);
                            let sample = *level_mailbox.latest.lock().unwrap();
                            deliver_level_sample(&level, sample);
                        }
                        StreamCommand::CaptureEnded => {
                            capture_ended = true;
                            break;
                        }
                        StreamCommand::Stop => {
                            stopped = true;
                            break;
                        }
                    }
                }
                if (stopped || capture_ended)
                    && let Err(err) = child.kill()
                    && err.kind() != std::io::ErrorKind::InvalidInput
                {
                    warn!(err = %err, "failed to stop parec capture");
                }
                let status = child.wait();
                if reader.join().is_err() {
                    error!("audio stream reader panicked");
                }
                let detail = join_stderr_reader(stderr_reader);
                if !stopped {
                    let message = match status {
                        Ok(status) => {
                            format!("audio input stopped: parec exited with {status}{detail}")
                        }
                        Err(err) => format!("audio input stopped: failed to wait for parec: {err}"),
                    };
                    warn!(err = %message, "error reading audio stream");
                    let mut current = stream_error.lock().unwrap();
                    if current.is_none() {
                        *current = Some(message);
                    }
                }
            })
            .map_err(|e| anyhow!("failed to spawn audio thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(StreamHandle { command_tx, thread }),
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

    /// Stops recording and returns the WAV-encoded data plus the file path it
    /// should be written to.
    pub fn stop(&self) -> Result<(Vec<u8>, PathBuf)> {
        let (samples, start_time) = self.stop_and_wait_samples()?;
        let bytes_captured = samples.len().saturating_mul(2);
        let wav = self
            .encode_samples_to_wav(samples)
            .map_err(|e| anyhow!("failed to encode to WAV: {e}"))?;

        let path = get_path_to_recording(start_time)?;

        info!(bytes_captured, "recording stopped");
        Ok((wav, path))
    }

    fn stop_and_wait_samples(&self) -> Result<(Vec<f32>, DateTime<Local>)> {
        self.reject_owner_thread_control()?;
        let _control = self.control.lock().unwrap();
        self.stop_and_wait_samples_locked()
    }

    fn stop_and_wait_samples_locked(&self) -> Result<(Vec<f32>, DateTime<Local>)> {
        self.reject_owner_thread_control()?;
        let mut inner = self.inner.lock().unwrap();
        if inner.stream.is_none() {
            bail!("recorder is not recording");
        }
        inner.state = RecorderState::Stopped;

        let stream = inner.stream.take();
        let start_time = inner.start_time;
        drop(inner);

        // Stop accepting before signalling the owner thread. Never join while
        // holding a lock the callback may need.
        self.capture.lock().unwrap().accepting = false;

        if let Some(handle) = stream {
            let _ = handle.command_tx.send(StreamCommand::Stop);
            if handle.thread.join().is_err() {
                bail!("failed to close audio stream: audio thread panicked");
            }
        }

        let samples = std::mem::take(&mut self.capture.lock().unwrap().buffer);
        Ok((samples, start_time))
    }

    /// Stops capture and releases the device without PCM/WAV conversion.
    pub fn cancel(&self) -> Result<()> {
        let (samples, _) = self.stop_and_wait_samples()?;
        drop(samples);
        Ok(())
    }

    pub fn get_recording_duration(&self) -> Duration {
        let inner = self.inner.lock().unwrap();
        if inner.state != RecorderState::Recording {
            return Duration::ZERO;
        }
        inner.start_instant.elapsed()
    }

    /// Takes the first asynchronous device error reported by CPAL, if any.
    pub fn take_error(&self) -> Option<String> {
        self.stream_error.lock().unwrap().take()
    }

    pub fn encode_to_wav(&self, raw: &[u8]) -> Result<Vec<u8>> {
        if raw.is_empty() {
            bail!("no audio data to encode");
        }

        let mut out = self.wav_header(raw.len())?;
        out.extend_from_slice(raw);
        Ok(out)
    }

    fn encode_samples_to_wav(&self, samples: Vec<f32>) -> Result<Vec<u8>> {
        if samples.is_empty() {
            bail!("no audio data to encode");
        }
        let data_size = samples
            .len()
            .checked_mul(2)
            .ok_or_else(|| anyhow!("audio data exceeds the WAV size limit"))?;
        let mut out = self.wav_header(data_size)?;
        for sample in samples {
            let int_sample = (sample * 32767.0) as i16;
            out.extend_from_slice(&int_sample.to_le_bytes());
        }
        Ok(out)
    }

    fn wav_header(&self, data_len: usize) -> Result<Vec<u8>> {
        let num_channels = u16::try_from(self.config.channels)
            .map_err(|_| anyhow!("audio channel count does not fit u16"))?;
        let sample_rate = u32::try_from(self.config.sample_rate)
            .map_err(|_| anyhow!("audio sample rate does not fit u32"))?;
        let bits_per_sample = u16::try_from(self.config.bit_depth)
            .map_err(|_| anyhow!("audio bit depth does not fit u16"))?;
        let byte_rate = sample_rate
            .checked_mul(u32::from(num_channels))
            .and_then(|rate| rate.checked_mul(u32::from(bits_per_sample)))
            .map(|rate| rate / 8)
            .ok_or_else(|| anyhow!("WAV byte rate overflows u32"))?;
        let block_align = num_channels
            .checked_mul(bits_per_sample)
            .map(|align| align / 8)
            .ok_or_else(|| anyhow!("WAV block alignment overflows u16"))?;
        let data_size = u32::try_from(data_len)
            .map_err(|_| anyhow!("audio data exceeds the WAV size limit"))?;
        let riff_size = data_size
            .checked_add(36)
            .ok_or_else(|| anyhow!("audio data exceeds the WAV size limit"))?;

        let capacity = 44usize
            .checked_add(data_len)
            .ok_or_else(|| anyhow!("audio data exceeds the WAV size limit"))?;
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&num_channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&bits_per_sample.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_size.to_le_bytes());
        Ok(out)
    }

    /// Returns true if the recording has exceeded the maximum duration.
    pub fn has_timed_out(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        if inner.state != RecorderState::Recording {
            return false;
        }
        inner.start_instant.elapsed()
            >= Duration::from_secs(self.config.max_duration_min.max(0) as u64 * 60)
    }

    pub fn close(&self) -> Result<()> {
        self.reject_owner_thread_control()?;
        let _control = self.control.lock().unwrap();
        if self.inner.lock().unwrap().stream.is_some() {
            warn!("recorder still active during close, stopping recording");
            if let Err(err) = self.stop_and_wait_samples_locked() {
                error!(err = %err, "error stopping recording during close");
                return Err(err);
            }
        }
        debug!("audio recorder closed");
        Ok(())
    }

    fn reject_owner_thread_control(&self) -> Result<()> {
        if self
            .owner_thread
            .lock()
            .unwrap()
            .is_some_and(|id| id == std::thread::current().id())
        {
            bail!("cannot control recorder from its audio stream thread");
        }
        Ok(())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // `Drop` has exclusive access, so recover poisoned state directly. The
        // stream callback owns another command sender; merely dropping our
        // sender would not wake the owner thread.
        let inner = match self.inner.get_mut() {
            Ok(inner) => inner,
            Err(poisoned) => poisoned.into_inner(),
        };
        inner.state = RecorderState::Stopped;
        let stream = inner.stream.take();

        let mut capture = match self.capture.lock() {
            Ok(capture) => capture,
            Err(poisoned) => poisoned.into_inner(),
        };
        capture.accepting = false;
        drop(capture);

        if let Some(handle) = stream {
            let _ = handle.command_tx.send(StreamCommand::Stop);
            if handle.thread.thread().id() == std::thread::current().id() {
                // The owner will observe Stop after the current callback returns.
                return;
            }
            if handle.thread.join().is_err() {
                error!("audio stream thread panicked during recorder drop");
            }
        }
    }
}

fn max_samples(config: &AudioConfig) -> Result<usize> {
    let sample_rate = usize::try_from(config.sample_rate)
        .map_err(|_| anyhow!("audio sample rate does not fit usize"))?;
    let minutes = usize::try_from(config.max_duration_min)
        .map_err(|_| anyhow!("audio max duration does not fit usize"))?;
    sample_rate
        .checked_mul(60)
        .and_then(|per_minute| per_minute.checked_mul(minutes))
        .ok_or_else(|| anyhow!("maximum audio sample count overflows usize"))
}

/// Converts an owned float32 buffer to little-endian int16 PCM.
#[cfg(test)]
fn samples_to_pcm(samples: Vec<f32>) -> Vec<u8> {
    let mut data = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        // convert float32 (-1.0 to 1.0) to int16 (-32768 to 32767)
        let int_sample = (sample * 32767.0) as i16;
        data.extend_from_slice(&int_sample.to_le_bytes());
    }
    data
}

fn capture_samples(capture: &Mutex<CaptureState>, samples: &[f32]) {
    let mut capture = capture.lock().unwrap();
    if !capture.accepting {
        return;
    }
    let remaining = capture.max_samples.saturating_sub(capture.buffer.len());
    let keep = samples.len().min(remaining);
    capture.buffer.extend_from_slice(&samples[..keep]);
    if capture.buffer.len() == capture.max_samples {
        capture.accepting = false;
    }
}

fn read_parec_stream(
    mut stdout: impl Read,
    frames_per_block: usize,
    capture: &Mutex<CaptureState>,
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
        capture_samples(capture, &samples);
        queue_level_sample(level, mailbox, command_tx, &samples);
    }
}

/// Requests a small server-side buffer so `kill` on stop loses at most ~50 ms.
/// PulseAudio's default record latency is about one second, which truncated
/// recordings to whole-second multiples and dropped anything shorter.
fn parec_command(source_name: &str, sample_rate: &str) -> Command {
    let mut command = Command::new("parec");
    command
        .args([
            "--record",
            "--raw",
            "--format=float32le",
            "--channels=1",
            "--client-name=dictator",
            "--stream-name=Dictation",
            "--latency-msec=50",
            "--process-time-msec=10",
            "--device",
            source_name,
            "--rate",
            sample_rate,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
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

fn deliver_level_sample(level: &Mutex<LevelState>, sample: LevelSample) {
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

pub fn write_audio_data(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if !dir.exists() {
        bail!("directory does not exist: {}", dir.display());
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);

    let mut file = options
        .open(path)
        .map_err(|e| anyhow!("failed to open audio file: {e}"))?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| anyhow!("failed to restrict audio file permissions: {e}"))?;
    file.write_all(data)
        .map_err(|e| anyhow!("failed to write audio data: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AudioConfig {
        AudioConfig {
            sample_rate: 16000,
            channels: 1,
            bit_depth: 16,
            frames_per_block: 1024,
            max_duration_min: 5,
        }
    }

    #[test]
    fn wav_header_matches_go_layout() {
        let recorder = Recorder::for_test(cfg());
        let wav = recorder.encode_to_wav(&[1, 0, 2, 0]).unwrap();
        assert_eq!(wav.len(), 48);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 40);
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(wav[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16000);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32000);
        assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
        assert_eq!(&wav[44..], &[1, 0, 2, 0]);
        assert!(recorder.encode_to_wav(&[]).is_err());
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
    fn pcm_conversion() {
        let data = samples_to_pcm(vec![0.0, 1.0, -1.0]);
        assert_eq!(data, vec![0, 0, 0xFF, 0x7F, 0x01, 0x80]);
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
    fn parec_command_captures_the_resolved_source_directly() {
        let command = parec_command("bluez_input.headset", "16000");
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
                "--device",
                "bluez_input.headset",
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
    #[ignore = "live microphone probe"]
    fn live_capture_start_cancel_probe() {
        let recorder = Recorder::for_test(cfg());
        recorder.start().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let samples_captured = loop {
            let samples = recorder.capture.lock().unwrap().buffer.len();
            if samples > 0 || Instant::now() >= deadline {
                break samples;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let capture_error = recorder.take_error();
        recorder.cancel().unwrap();
        assert_eq!(recorder.get_state(), RecorderState::Stopped);
        assert!(capture_error.is_none(), "capture failed: {capture_error:?}");
        assert!(
            samples_captured > 0,
            "parec produced no float32 PCM samples within 3 seconds"
        );
    }

    #[test]
    fn capture_never_exceeds_preallocated_sample_bound() {
        let capture = Mutex::new(CaptureState {
            buffer: Vec::with_capacity(4),
            accepting: true,
            max_samples: 4,
        });
        let original_capacity = capture.lock().unwrap().buffer.capacity();

        for _ in 0..100 {
            capture_samples(&capture, &[0.0; 1024]);
        }

        let capture = capture.lock().unwrap();
        assert_eq!(capture.buffer.len(), 4);
        assert_eq!(capture.buffer.capacity(), original_capacity);
        assert!(!capture.accepting);
    }

    #[test]
    fn cancel_joins_synthetic_stream_and_discards_capture() {
        let recorder = Recorder::for_test(cfg());
        let (command_tx, command_rx) = mpsc::sync_channel(2);
        let capture = Arc::clone(&recorder.capture);
        let thread = std::thread::spawn(move || {
            let _capture_owner = capture;
            while let Ok(command) = command_rx.recv() {
                if matches!(command, StreamCommand::Stop) {
                    break;
                }
            }
        });
        {
            let mut inner = recorder.inner.lock().unwrap();
            inner.state = RecorderState::Recording;
            inner.stream = Some(StreamHandle { command_tx, thread });
        }
        {
            let mut capture = recorder.capture.lock().unwrap();
            capture.accepting = true;
            capture.buffer.extend_from_slice(&[0.25, -0.25]);
        }

        recorder.cancel().unwrap();

        assert_eq!(recorder.get_state(), RecorderState::Stopped);
        let capture = recorder.capture.lock().unwrap();
        assert!(capture.buffer.is_empty());
        assert!(!capture.accepting);
    }

    #[test]
    fn observer_cannot_reentrantly_stop_or_discard_recording() {
        let recorder = Arc::new(Recorder::for_test(cfg()));
        let (result_tx, result_rx) = mpsc::channel();
        let weak_recorder = Arc::downgrade(&recorder);
        recorder.set_level_observer(
            Some(Arc::new(move |_| {
                let err = weak_recorder
                    .upgrade()
                    .expect("recorder remains alive")
                    .stop()
                    .unwrap_err()
                    .to_string();
                result_tx.send(err).unwrap();
            })),
            Duration::ZERO,
        );

        let (command_tx, command_rx) = mpsc::sync_channel(2);
        let (invoke_tx, invoke_rx) = mpsc::channel();
        let level = Arc::clone(&recorder.level);
        let owner_thread = Arc::clone(&recorder.owner_thread);
        let thread = std::thread::spawn(move || {
            let _owner_registration = OwnerThreadRegistration::new(owner_thread);
            invoke_rx.recv().unwrap();
            deliver_level_sample(&level, LevelSample::default());
            while let Ok(command) = command_rx.recv() {
                if matches!(command, StreamCommand::Stop) {
                    break;
                }
            }
        });
        {
            let mut inner = recorder.inner.lock().unwrap();
            inner.state = RecorderState::Recording;
            inner.stream = Some(StreamHandle { command_tx, thread });
        }
        {
            let mut capture = recorder.capture.lock().unwrap();
            capture.accepting = true;
            capture.buffer.extend_from_slice(&[0.25, -0.25]);
        }

        invoke_tx.send(()).unwrap();
        let err = result_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        assert_eq!(err, "cannot control recorder from its audio stream thread");
        assert_eq!(recorder.get_state(), RecorderState::Recording);
        assert!(recorder.inner.lock().unwrap().stream.is_some());
        let capture = recorder.capture.lock().unwrap();
        assert_eq!(capture.buffer, [0.25, -0.25]);
        assert!(capture.accepting);
        drop(capture);
        recorder.cancel().unwrap();
    }

    #[test]
    fn observer_reentrant_stop_does_not_deadlock_external_stop() {
        let recorder = Arc::new(Recorder::for_test(cfg()));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (proceed_tx, proceed_rx) = mpsc::channel();
        let proceed_rx = Arc::new(Mutex::new(proceed_rx));
        let (result_tx, result_rx) = mpsc::channel();
        let weak_recorder = Arc::downgrade(&recorder);
        recorder.set_level_observer(
            Some(Arc::new(move |_| {
                entered_tx.send(()).unwrap();
                proceed_rx.lock().unwrap().recv().unwrap();
                let err = weak_recorder
                    .upgrade()
                    .expect("recorder remains alive")
                    .stop()
                    .unwrap_err()
                    .to_string();
                result_tx.send(err).unwrap();
            })),
            Duration::ZERO,
        );

        let (command_tx, command_rx) = mpsc::sync_channel(2);
        let (invoke_tx, invoke_rx) = mpsc::channel();
        let level = Arc::clone(&recorder.level);
        let owner_thread = Arc::clone(&recorder.owner_thread);
        let thread = std::thread::spawn(move || {
            let _owner_registration = OwnerThreadRegistration::new(owner_thread);
            invoke_rx.recv().unwrap();
            deliver_level_sample(&level, LevelSample::default());
            while let Ok(command) = command_rx.recv() {
                if matches!(command, StreamCommand::Stop) {
                    break;
                }
            }
        });
        {
            let mut inner = recorder.inner.lock().unwrap();
            inner.state = RecorderState::Recording;
            inner.stream = Some(StreamHandle { command_tx, thread });
        }

        invoke_tx.send(()).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let stopping_recorder = Arc::clone(&recorder);
        let external_stop = std::thread::spawn(move || stopping_recorder.cancel());

        let deadline = Instant::now() + Duration::from_secs(1);
        while recorder.inner.lock().unwrap().stream.is_some() {
            assert!(
                Instant::now() < deadline,
                "external stop did not reach join"
            );
            std::thread::yield_now();
        }
        proceed_tx.send(()).unwrap();

        let err = result_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(err, "cannot control recorder from its audio stream thread");
        external_stop.join().unwrap().unwrap();
        assert_eq!(recorder.get_state(), RecorderState::Stopped);
    }

    #[test]
    fn dropping_active_recorder_releases_callback_state() {
        let recorder = Recorder::for_test(cfg());
        let weak_capture = Arc::downgrade(&recorder.capture);
        let (command_tx, command_rx) = mpsc::sync_channel(2);
        let capture = Arc::clone(&recorder.capture);
        let thread = std::thread::spawn(move || {
            let _capture_owner = capture;
            while let Ok(command) = command_rx.recv() {
                if matches!(command, StreamCommand::Stop) {
                    break;
                }
            }
        });
        {
            let mut inner = recorder.inner.lock().unwrap();
            inner.state = RecorderState::Recording;
            inner.stream = Some(StreamHandle { command_tx, thread });
        }

        drop(recorder);

        assert!(weak_capture.upgrade().is_none());
    }

    #[test]
    fn dropping_recorder_recovers_poisoned_control_state() {
        let recorder = Recorder::for_test(cfg());
        let weak_capture = Arc::downgrade(&recorder.capture);
        let (command_tx, command_rx) = mpsc::sync_channel(2);
        let capture = Arc::clone(&recorder.capture);
        let thread = std::thread::spawn(move || {
            let _capture_owner = capture;
            while let Ok(command) = command_rx.recv() {
                if matches!(command, StreamCommand::Stop) {
                    break;
                }
            }
        });
        {
            let mut inner = recorder.inner.lock().unwrap();
            inner.state = RecorderState::Recording;
            inner.stream = Some(StreamHandle { command_tx, thread });
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _inner = recorder.inner.lock().unwrap();
            panic!("poison recorder state");
        }));

        drop(recorder);

        assert!(weak_capture.upgrade().is_none());
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
}
