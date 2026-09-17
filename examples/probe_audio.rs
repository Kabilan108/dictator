//! Lists the default input device and captures a short sample to verify the
//! audio backend works on this machine.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

fn main() {
    let host = cpal::default_host();
    println!("host: {:?}", host.id());
    let device = host
        .default_input_device()
        .expect("no default input device");
    println!("default input: {}", device.name().unwrap_or_default());
    let cfg = device.default_input_config().expect("default input config");
    println!("default config: {cfg:?}");

    let frames = Arc::new(Mutex::new(0usize));
    let peak = Arc::new(Mutex::new(0f32));
    let (f, p) = (frames.clone(), peak.clone());
    let stream = device
        .build_input_stream(
            &cpal::StreamConfig {
                channels: 1,
                sample_rate: cpal::SampleRate(16000),
                buffer_size: cpal::BufferSize::Fixed(1024),
            },
            move |data: &[f32], _| {
                *f.lock().unwrap() += data.len();
                let mut pk = p.lock().unwrap();
                for s in data {
                    *pk = pk.max(s.abs());
                }
            },
            |e| eprintln!("stream error: {e}"),
            None,
        )
        .expect("build stream");
    stream.play().expect("play");
    std::thread::sleep(Duration::from_millis(1500));
    drop(stream);
    println!(
        "captured {} frames in 1.5s, peak {:.4}",
        frames.lock().unwrap(),
        peak.lock().unwrap()
    );
}
