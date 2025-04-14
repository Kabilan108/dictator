// src-tauri/src/audio.rs
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, SupportedStreamConfig, SupportedStreamConfigsError};
use hound;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use thiserror::Error;
// REMOVE: use std::fs::File; // No longer needed here

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("No default input device found")]
    NoDefaultInputDevice,
    #[error("No supported input config found")]
    NoSupportedConfig,
    #[error("Failed to get supported input configs: {0}")]
    SupportedConfigsError(#[from] SupportedStreamConfigsError),
    #[error("Failed to build input stream: {0}")]
    BuildStreamError(#[from] cpal::BuildStreamError),
    #[error("Failed to play stream: {0}")]
    PlayStreamError(#[from] cpal::PlayStreamError),
    #[error("Failed to pause stream: {0}")]
    PauseStreamError(#[from] cpal::PauseStreamError),
    #[error("WAV Error: {0}")]
    WavError(#[from] hound::Error),
    #[error("IO Error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Recording is already in progress")]
    AlreadyRecording,
    #[error("Not currently recording")]
    NotRecording,
    #[error("Audio buffer is unexpectedly empty")]
    BufferEmpty,
    #[error("Audio recorder components (device/config) not initialized")]
    NotInitialized,
}

// State shared safely with the audio callback
#[derive(Default)]
struct RecorderSharedState {
    buffer: Vec<f32>,
    is_recording: bool,
}

// --- Wrapper for cpal::Stream to mark it Send + Sync ---
// This is necessary because cpal::Stream itself is not Send/Sync.
struct SendStream(Stream);

// SAFETY: We assert that Send and Sync are safe for our specific usage pattern.
// The Stream object is created, played, and paused only from the Tauri command context.
// The audio callback, running on a separate thread, only interacts with the
// Arc<Mutex<RecorderSharedState>>, not the Stream object itself.
// The Stream object is contained within a Mutex in AudioRecorder, ensuring exclusive access
// during play/pause/drop operations from the command context.
unsafe impl Send for SendStream {}
unsafe impl Sync for SendStream {}
// --- End Wrapper ---


// Main struct, needs to be Send + Sync
pub struct AudioRecorder {
    shared_state: Arc<Mutex<RecorderSharedState>>,
    // Store device and config details needed for stream creation
    // These might also not be Send/Sync depending on backend, so store basic info if needed
    // For now, let's assume Device/SupportedStreamConfig are okay if not sent across threads directly
    // If errors persist related to these, we'll need to store device name (String) etc.
    device: Device,
    config: SupportedStreamConfig,
    // Use the wrapper type here
    active_stream: Mutex<Option<SendStream>>,
}


impl AudioRecorder {
    pub fn new() -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(AudioError::NoDefaultInputDevice)?;
        let device_name = device.name().unwrap_or_else(|_| "Unknown".into()); // Get name for logging
        log::info!("Using default input device: {}", device_name);

        let supported_configs = device.supported_input_configs()?;
        let target_sr = cpal::SampleRate(16000);
        let target_channels = 1;

        let config = supported_configs
            .filter(|c| c.sample_format() == SampleFormat::F32 || c.sample_format() == SampleFormat::I16)
            .find(|c| c.min_sample_rate() <= target_sr && target_sr <= c.max_sample_rate() && c.channels() == target_channels)
            .map(|c| c.with_sample_rate(target_sr))
            .or_else(|| {
                device.supported_input_configs().ok()?
                    .filter(|c| c.sample_format() == SampleFormat::F32 || c.sample_format() == SampleFormat::I16)
                    .find(|c| c.channels() == target_channels)
                    .map(|c| c.with_max_sample_rate())
            })
            .ok_or(AudioError::NoSupportedConfig)?;

        log::info!("Selected input config: {:?}", config);

        Ok(Self {
            shared_state: Arc::new(Mutex::new(RecorderSharedState::default())),
            device, // Store the actual device for now
            config, // Store the actual config for now
            active_stream: Mutex::new(None),
        })
    }

    pub fn start_recording(&self) -> Result<(), AudioError> {
        let mut stream_guard = self.active_stream.lock().unwrap();
        if stream_guard.is_some() {
            return Err(AudioError::AlreadyRecording);
        }

        let mut shared_state_guard = self.shared_state.lock().unwrap();
        if shared_state_guard.is_recording {
             return Err(AudioError::AlreadyRecording);
        }
        shared_state_guard.buffer.clear();
        shared_state_guard.is_recording = true;
        drop(shared_state_guard); // Release lock early

        let shared_state_clone = self.shared_state.clone();

        let err_fn = |err| {
            log::error!("An error occurred on the audio stream: {}", err);
        };

        let config_ref = &self.config;

        let stream = match config_ref.sample_format() {
             SampleFormat::F32 => self.device.build_input_stream(
                &config_ref.config(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mut state = shared_state_clone.lock().unwrap();
                    if state.is_recording {
                        state.buffer.extend_from_slice(data);
                    }
                },
                err_fn,
                None,
            )?,
            SampleFormat::I16 => self.device.build_input_stream(
                &config_ref.config(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let mut state = shared_state_clone.lock().unwrap();
                    if state.is_recording {
                        let samples_f32: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                        state.buffer.extend_from_slice(&samples_f32);
                    }
                },
                err_fn,
                None,
            )?,
            _ => return Err(AudioError::NoSupportedConfig),
        };

        stream.play()?;
        *stream_guard = Some(SendStream(stream)); // Store the wrapped stream

        log::info!("Recording started.");
        Ok(())
    }

    pub fn stop_recording(&self, output_path: PathBuf) -> Result<(), AudioError> {
        let stream_wrapper = {
            let mut stream_guard = self.active_stream.lock().unwrap();
            stream_guard.take().ok_or(AudioError::NotRecording)?
        };

        // Access the inner stream to pause and drop it
        stream_wrapper.0.pause()?;
        drop(stream_wrapper); // Drop the wrapper, which drops the inner stream
        log::info!("Audio stream stopped and dropped.");

        let buffer_copy = {
            let mut state = self.shared_state.lock().unwrap();
            state.is_recording = false;

            if state.buffer.is_empty() {
                log::warn!("Audio buffer is empty after recording.");
            }
            let buffer_copy = state.buffer.clone();
            state.buffer.clear();
            buffer_copy
        };

        log::info!("Stopping recording. Buffer size: {} samples", buffer_copy.len());

        // --- Write WAV file ---
        let spec = hound::WavSpec {
            channels: self.config.channels(),
            sample_rate: self.config.sample_rate().0,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        // Use std::fs::File directly here
        let file = std::fs::File::create(&output_path)?;
        let buf_writer = BufWriter::new(file);
        let mut wav_writer = hound::WavWriter::new(buf_writer, spec)?;

        for sample_f32 in buffer_copy {
            let sample_i16 = (sample_f32 * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            wav_writer.write_sample(sample_i16)?;
        }

        wav_writer.finalize()?;

        log::info!("Successfully wrote WAV file to: {:?}", output_path);
        Ok(())
    }

    // list_devices remains the same
    pub fn list_devices() -> Result<Vec<String>, String> {
         let host = cpal::default_host();
         let devices = host.input_devices().map_err(|e| e.to_string())?;
         let mut device_names = Vec::new();
         for device in devices {
             match device.name() {
                 Ok(name) => device_names.push(name),
                 Err(e) => log::warn!("Failed to get device name: {}", e),
             }
         }
         Ok(device_names)
     }
}

// Note: If errors persist related to Device or SupportedStreamConfig not being Send/Sync,
// you would need to modify AudioRecorder to store device name (String) and config
// parameters (u32, u16, SampleFormat) instead of the actual cpal objects. Then,
// in start_recording, you would re-acquire the Device by name and find the config again.
// This adds overhead but guarantees Send+Sync for the stored state.
