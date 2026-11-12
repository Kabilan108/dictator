//! The daemon's OSD socket, read on a thread that reconnects whenever it
//! drops. Each line becomes an [`Input`] stamped with when it arrived, and
//! the overlay is woken to take it.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::visual::Event;

use super::model::Input;

const RECONNECT_DELAY: Duration = Duration::from_millis(500);
/// About eight seconds of meter samples; older ones are dropped first.
const QUEUE_CAPACITY: usize = 256;

#[derive(Default)]
struct Shared {
    queue: VecDeque<(Instant, Input)>,
    waker: Option<rune::Waker>,
}

#[derive(Clone, Default)]
pub(super) struct Feed {
    shared: Arc<Mutex<Shared>>,
}

impl Feed {
    pub(super) fn start(path: PathBuf) -> Self {
        let feed = Self::default();
        let shared = Arc::downgrade(&feed.shared);
        thread::Builder::new()
            .name("dictator-osd-feed".into())
            .spawn(move || {
                while shared.strong_count() > 0 {
                    if let Ok(stream) = UnixStream::connect(&path) {
                        read(stream, &shared);
                        push(&shared, Input::Disconnected);
                    }
                    thread::sleep(RECONNECT_DELAY);
                }
            })
            .expect("failed to start the OSD feed thread");
        feed
    }

    pub(super) fn set_waker(&self, waker: rune::Waker) {
        self.shared.lock().unwrap().waker = Some(waker);
    }

    pub(super) fn drain(&self) -> Vec<(Instant, Input)> {
        self.shared.lock().unwrap().queue.drain(..).collect()
    }
}

fn read(stream: UnixStream, shared: &Weak<Mutex<Shared>>) {
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        if let Some(input) = parse(&line)
            && !push(shared, input)
        {
            return;
        }
    }
}

/// Queues `input` and wakes the overlay; false once the feed is gone.
fn push(shared: &Weak<Mutex<Shared>>, input: Input) -> bool {
    let Some(shared) = shared.upgrade() else {
        return false;
    };
    let waker = {
        let mut shared = shared.lock().unwrap();
        if shared.queue.len() >= QUEUE_CAPACITY {
            let meter = shared
                .queue
                .iter()
                .position(|(_, queued)| matches!(queued, Input::Meter { .. }));
            shared.queue.remove(meter.unwrap_or(0));
        }
        shared.queue.push_back((Instant::now(), input));
        shared.waker.clone()
    };
    if let Some(waker) = waker {
        waker();
    }
    true
}

/// Events the daemon does not send yet, prototyped by the demo feed.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum Extra {
    #[serde(rename = "x-transcript")]
    Transcript { text: String },
    #[serde(rename = "x-provider")]
    Provider { name: String },
}

pub(super) fn parse(line: &str) -> Option<Input> {
    if let Ok(event) = serde_json::from_str::<Event>(line) {
        return Some(match event {
            Event::State(state) => Input::State {
                value: state.value,
                duration_ms: state.recording_duration_ms,
                message: state.message,
            },
            Event::Meter(meter) => Input::Meter {
                rms: meter.rms as f32,
                peak: meter.peak as f32,
            },
        });
    }
    match serde_json::from_str::<Extra>(line).ok()? {
        Extra::Transcript { text } => Some(Input::Transcript(text)),
        Extra::Provider { name } if !name.trim().is_empty() => Some(Input::Provider(name)),
        Extra::Provider { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visual::StateValue;

    #[test]
    fn parses_daemon_events_and_the_prototype_extras() {
        let cases = [
            (
                r#"{"type":"state","value":"transcribing","recording_duration_ms":4200}"#,
                Some(Input::State {
                    value: StateValue::Transcribing,
                    duration_ms: Some(4200),
                    message: String::new(),
                }),
            ),
            (
                r#"{"type":"state","value":"error","message":"timed out"}"#,
                Some(Input::State {
                    value: StateValue::Error,
                    duration_ms: None,
                    message: "timed out".into(),
                }),
            ),
            (
                r#"{"type":"meter","rms":0.5,"peak":0.75}"#,
                Some(Input::Meter {
                    rms: 0.5,
                    peak: 0.75,
                }),
            ),
            (
                r#"{"type":"x-transcript","text":"hello"}"#,
                Some(Input::Transcript("hello".into())),
            ),
            (
                r#"{"type":"x-provider","name":"openai"}"#,
                Some(Input::Provider("openai".into())),
            ),
            (r#"{"type":"x-provider","name":" "}"#, None),
            (r#"{"type":"x-other"}"#, None),
            ("not json", None),
        ];
        for (line, expected) in cases {
            assert_eq!(parse(line), expected, "{line}");
        }
    }

    #[test]
    fn a_full_queue_drops_meter_samples_before_state() {
        let feed = Feed::default();
        let shared = Arc::downgrade(&feed.shared);
        push(&shared, Input::Provider("first".into()));
        for _ in 0..QUEUE_CAPACITY {
            push(
                &shared,
                Input::Meter {
                    rms: 0.1,
                    peak: 0.2,
                },
            );
        }
        let drained = feed.drain();
        assert_eq!(drained.len(), QUEUE_CAPACITY);
        assert_eq!(drained[0].1, Input::Provider("first".into()));
    }
}
