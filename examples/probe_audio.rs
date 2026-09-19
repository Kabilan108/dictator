//! Lists microphone priorities and observes 1.5 seconds of capture without
//! saving audio or sending it to a transcription provider.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use dictator::audio::{Recorder, microphone_preferences};
use dictator::utils::AudioConfig;

fn main() -> Result<()> {
    for microphone in microphone_preferences()?.microphones {
        println!(
            "{}: {} ({})",
            microphone.id,
            microphone.name,
            if microphone.connected {
                "connected"
            } else {
                "disconnected"
            }
        );
    }
    let recorder = Recorder::new(AudioConfig {
        sample_rate: 16_000,
        channels: 1,
        bit_depth: 16,
        frames_per_block: 1024,
        max_duration_min: 5,
    })?;
    let peak = Arc::new(Mutex::new(0.0_f64));
    let observed_peak = Arc::clone(&peak);
    recorder.set_level_observer(
        Some(Arc::new(move |sample| {
            let mut peak = observed_peak.lock().unwrap();
            *peak = peak.max(sample.peak);
        })),
        Duration::from_millis(20),
    );
    recorder.start()?;
    std::thread::sleep(Duration::from_millis(1500));
    let capture_error = recorder.take_error();
    recorder.cancel()?;
    if let Some(error) = capture_error {
        anyhow::bail!("{error}");
    }
    println!(
        "captured for 1.5s, peak {:.4}; audio discarded",
        peak.lock().unwrap()
    );
    Ok(())
}
