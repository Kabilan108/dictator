use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Local};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, BuildStreamError, SampleRate, StreamConfig};
use tracing::{debug, error, info, warn};

use crate::utils::{AudioConfig, get_path_to_recording};

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
    buffer: Vec<f32>,
    start_instant: Instant,
    start_time: DateTime<Local>,
    /// Thread that owns the cpal stream; sending on `stop_tx` (or dropping it)
    /// stops and closes the stream.
    stream: Option<StreamHandle>,
    /// Incremented on every start so a stale timeout thread can no-op.
    generation: u64,
}

struct StreamHandle {
    stop_tx: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

/// Records mono float32 audio from the default input device into memory and
/// encodes it as 16-bit PCM WAV on stop.
pub struct Recorder {
    config: AudioConfig,
    inner: Arc<Mutex<Inner>>,
    level: Arc<Mutex<LevelState>>,
    timeout_generation: Arc<AtomicU64>,
}

impl Recorder {
    pub fn new(config: AudioConfig) -> Result<Self> {
        // Probe the host so that a missing audio backend fails at startup, like
        // PortAudio initialization did.
        let host = cpal::default_host();
        debug!(host = ?host.id(), "audio host initialized");

        debug!(
            sr = config.sample_rate,
            channels = config.channels,
            bit_depth = config.bit_depth,
            "recorder initialized"
        );

        Ok(Self {
            config,
            inner: Arc::new(Mutex::new(Inner {
                state: RecorderState::Idle,
                buffer: Vec::new(),
                start_instant: Instant::now(),
                start_time: Local::now(),
                stream: None,
                generation: 0,
            })),
            level: Arc::new(Mutex::new(LevelState {
                observer: None,
                interval: Duration::ZERO,
                last_level_at: None,
            })),
            timeout_generation: Arc::new(AtomicU64::new(0)),
        })
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
        let mut inner = self.inner.lock().unwrap();

        if inner.state == RecorderState::Recording {
            bail!("recorder is already recording");
        }

        debug!("starting audio recording");

        inner.buffer = Vec::new();
        inner.start_instant = Instant::now();
        inner.start_time = Local::now();
        inner.generation += 1;
        let generation = inner.generation;
        self.level.lock().unwrap().last_level_at = None;

        let stream = match self.open_stream() {
            Ok(stream) => stream,
            Err(err) => {
                error!(err = %err, "failed to open audio stream");
                return Err(err);
            }
        };

        inner.stream = Some(stream);
        inner.state = RecorderState::Recording;
        drop(inner);

        self.spawn_timeout(generation);

        info!(
            max_duration_min = self.config.max_duration_min,
            "recording started"
        );
        Ok(())
    }

    fn spawn_timeout(&self, generation: u64) {
        self.timeout_generation.store(generation, Ordering::SeqCst);
        let max_duration = Duration::from_secs(self.config.max_duration_min.max(0) as u64 * 60);
        let recorder = self.clone_handle();
        std::thread::spawn(move || {
            let deadline = Instant::now() + max_duration;
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(250));
                if recorder.timeout_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
            }
            if recorder.timeout_generation.load(Ordering::SeqCst) != generation {
                return;
            }
            recorder.stop_recording_due_to_timeout();
        });
    }

    fn clone_handle(&self) -> Recorder {
        Recorder {
            config: self.config.clone(),
            inner: Arc::clone(&self.inner),
            level: Arc::clone(&self.level),
            timeout_generation: Arc::clone(&self.timeout_generation),
        }
    }

    /// Opens the default input stream on a dedicated thread. Retries once after
    /// re-querying the host if the default device is missing or unavailable.
    fn open_stream(&self) -> Result<StreamHandle> {
        match self.try_open_stream() {
            Ok(handle) => Ok(handle),
            Err(err) if is_device_error(&err) => {
                warn!(err = %err, "refreshing audio host after input device failure");
                self.try_open_stream()
            }
            Err(err) => Err(err),
        }
    }

    fn try_open_stream(&self) -> Result<StreamHandle> {
        let config = StreamConfig {
            channels: 1,
            sample_rate: SampleRate(self.config.sample_rate.max(1) as u32),
            buffer_size: BufferSize::Fixed(self.config.frames_per_block.max(1) as u32),
        };

        let inner = Arc::clone(&self.inner);
        let level = Arc::clone(&self.level);
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();

        let thread = std::thread::Builder::new()
            .name("dictator-audio".into())
            .spawn(move || {
                let host = cpal::default_host();
                let Some(device) = host.default_input_device() else {
                    let _ = ready_tx.send(Err(anyhow!(
                        "failed to open audio stream: no default input device"
                    )));
                    return;
                };

                let data_inner = Arc::clone(&inner);
                let data_level = Arc::clone(&level);
                let stream = device.build_input_stream(
                    &config,
                    move |data: &[f32], _| {
                        publish_level_sample(&data_level, data);
                        let mut guard = data_inner.lock().unwrap();
                        if guard.state == RecorderState::Recording {
                            guard.buffer.extend_from_slice(data);
                        }
                    },
                    |err| warn!(err = %err, "error reading audio stream"),
                    None,
                );

                let stream = match stream {
                    Ok(stream) => stream,
                    Err(err) => {
                        let _ = ready_tx.send(Err(anyhow!("failed to open audio stream: {err}")));
                        return;
                    }
                };

                if let Err(err) = stream.play() {
                    let _ = ready_tx.send(Err(anyhow!("failed to start audio stream: {err}")));
                    return;
                }

                let _ = ready_tx.send(Ok(()));

                // Block until asked to stop (or the sender is dropped).
                let _ = stop_rx.recv();
                if let Err(err) = stream.pause() {
                    warn!(err = %err, "failed to stop audio stream");
                }
                drop(stream);
            })
            .map_err(|e| anyhow!("failed to spawn audio thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(StreamHandle { stop_tx, thread }),
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
        let (data, start_time) = self.stop_and_wait()?;

        let wav = self
            .encode_to_wav(&data)
            .map_err(|e| anyhow!("failed to encode to WAV: {e}"))?;

        let path = get_path_to_recording(start_time)?;

        info!(bytes_captured = data.len(), "recording stopped");
        Ok((wav, path))
    }

    fn stop_and_wait(&self) -> Result<(Vec<u8>, DateTime<Local>)> {
        let mut inner = self.inner.lock().unwrap();
        if inner.stream.is_none() {
            bail!("recorder is not recording");
        }
        inner.state = RecorderState::Stopped;
        // invalidate the pending timeout
        self.timeout_generation.fetch_add(1, Ordering::SeqCst);

        let stream = inner.stream.take();
        let start_time = inner.start_time;
        drop(inner);

        if let Some(handle) = stream {
            let _ = handle.stop_tx.send(());
            if handle.thread.join().is_err() {
                bail!("failed to close audio stream: audio thread panicked");
            }
        }

        let mut inner = self.inner.lock().unwrap();
        let data = stop_recording_locked(&mut inner);
        Ok((data, start_time))
    }

    pub fn get_recording_duration(&self) -> Duration {
        let inner = self.inner.lock().unwrap();
        if inner.state != RecorderState::Recording {
            return Duration::ZERO;
        }
        inner.start_instant.elapsed()
    }

    pub fn encode_to_wav(&self, raw: &[u8]) -> Result<Vec<u8>> {
        if raw.is_empty() {
            bail!("no audio data to encode");
        }

        let num_channels = self.config.channels as u16;
        let sample_rate = self.config.sample_rate as u32;
        let bits_per_sample = self.config.bit_depth as u16;
        let byte_rate = sample_rate * num_channels as u32 * bits_per_sample as u32 / 8;
        let block_align = num_channels * bits_per_sample / 8;
        let data_size = raw.len() as u32;

        let mut out = Vec::with_capacity(44 + raw.len());
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_size).to_le_bytes());
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
        out.extend_from_slice(raw);
        Ok(out)
    }

    fn stop_recording_due_to_timeout(&self) {
        warn!(
            max_duration_min = self.config.max_duration_min,
            "recording stopped due to timeout"
        );
        match self.stop_and_wait() {
            Ok((data, _)) => info!(bytes_captured = data.len(), "timeout stop completed"),
            Err(err) => error!(err = %err, "error during timeout stop"),
        }
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
        if self.is_recording() {
            warn!("recorder still active during close, stopping recording");
            if let Err(err) = self.stop_and_wait() {
                error!(err = %err, "error stopping recording during close");
                return Err(err);
            }
        }
        debug!("audio recorder closed");
        Ok(())
    }
}

fn is_device_error(err: &anyhow::Error) -> bool {
    if let Some(build) = err.downcast_ref::<BuildStreamError>() {
        return matches!(build, BuildStreamError::DeviceNotAvailable);
    }
    let msg = err.to_string();
    msg.contains("no default input device") || msg.contains("no longer available")
}

/// Converts the float32 buffer to little-endian int16 PCM and clears it.
fn stop_recording_locked(inner: &mut Inner) -> Vec<u8> {
    inner.state = RecorderState::Stopped;
    let mut data = Vec::with_capacity(inner.buffer.len() * 2);
    for sample in inner.buffer.iter() {
        // convert float32 (-1.0 to 1.0) to int16 (-32768 to 32767)
        let int_sample = (sample * 32767.0) as i16;
        data.extend_from_slice(&int_sample.to_le_bytes());
    }
    inner.buffer = Vec::new();
    data
}

fn publish_level_sample(level: &Mutex<LevelState>, samples: &[f32]) {
    let observer = {
        let mut guard = level.lock().unwrap();
        let Some(observer) = guard.observer.clone() else {
            return;
        };
        let now = Instant::now();
        if guard.interval > Duration::ZERO
            && let Some(last) = guard.last_level_at
            && now.duration_since(last) < guard.interval
        {
            return;
        }
        guard.last_level_at = Some(now);
        observer
    };
    observer(calculate_level(samples));
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
    std::fs::write(path, data).map_err(|e| anyhow!("failed to write audio data: {e}"))
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
        let recorder = Recorder {
            config: cfg(),
            inner: Arc::new(Mutex::new(Inner {
                state: RecorderState::Idle,
                buffer: Vec::new(),
                start_instant: Instant::now(),
                start_time: Local::now(),
                stream: None,
                generation: 0,
            })),
            level: Arc::new(Mutex::new(LevelState {
                observer: None,
                interval: Duration::ZERO,
                last_level_at: None,
            })),
            timeout_generation: Arc::new(AtomicU64::new(0)),
        };
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
        let mut inner = Inner {
            state: RecorderState::Recording,
            buffer: vec![0.0, 1.0, -1.0],
            start_instant: Instant::now(),
            start_time: Local::now(),
            stream: None,
            generation: 0,
        };
        let data = stop_recording_locked(&mut inner);
        assert_eq!(data, vec![0, 0, 0xFF, 0x7F, 0x01, 0x80]);
        assert!(inner.buffer.is_empty());
        assert_eq!(inner.state, RecorderState::Stopped);
    }
}
