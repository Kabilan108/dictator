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

pub(super) fn parse(line: &str) -> Option<Input> {
    Some(match serde_json::from_str::<Event>(line).ok()? {
        Event::State(state) => Input::State(state),
        Event::Meter(meter) => Input::Meter {
            rms: meter.rms as f32,
            peak: meter.peak as f32,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visual::{StateValue, new_state_event};

    #[test]
    fn parses_daemon_events() {
        let cases = [
            (
                r#"{"type":"state","value":"recording","recording_duration_ms":0,"provider":"openai"}"#,
                Some(Input::State(
                    new_state_event(StateValue::Recording, Some(Duration::ZERO), "")
                        .with_provider("openai"),
                )),
            ),
            (
                r#"{"type":"state","value":"typing","text":"hello"}"#,
                Some(Input::State(
                    new_state_event(StateValue::Typing, None, "").with_text("hello"),
                )),
            ),
            (
                r#"{"type":"state","value":"error","message":"timed out"}"#,
                Some(Input::State(new_state_event(
                    StateValue::Error,
                    None,
                    "timed out",
                ))),
            ),
            (
                r#"{"type":"meter","rms":0.5,"peak":0.75}"#,
                Some(Input::Meter {
                    rms: 0.5,
                    peak: 0.75,
                }),
            ),
            (r#"{"type":"other"}"#, None),
            ("not json", None),
        ];
        for (line, expected) in cases {
            assert_eq!(parse(line), expected, "{line}");
        }
    }

    /// The daemon's runtime directory, socket included, goes away while it is
    /// stopped and comes back when it restarts.
    #[test]
    fn reconnects_when_the_daemon_comes_back_in_a_new_directory() {
        use std::io::Write;
        use std::os::unix::net::UnixListener;

        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("dictator");
        let path = dir.join("osd.sock");
        let serve = |line: &str| {
            std::fs::create_dir(&dir).unwrap();
            let listener = UnixListener::bind(&path).unwrap();
            let line = format!("{line}\n");
            thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.write_all(line.as_bytes()).unwrap();
            })
        };
        let wait_for = |feed: &Feed, count: usize| {
            let mut inputs = Vec::new();
            for _ in 0..100 {
                inputs.extend(feed.drain().into_iter().map(|(_, input)| input));
                if inputs.len() >= count {
                    break;
                }
                thread::sleep(Duration::from_millis(30));
            }
            inputs
        };

        let first = serve(r#"{"type":"state","value":"recording"}"#);
        let feed = Feed::start(path.clone());
        first.join().unwrap();
        let inputs = wait_for(&feed, 2);
        assert_eq!(inputs.last(), Some(&Input::Disconnected), "{inputs:?}");
        std::fs::remove_dir_all(&dir).unwrap();

        let second = serve(r#"{"type":"state","value":"idle"}"#);
        second.join().unwrap();
        let inputs = wait_for(&feed, 2);
        assert_eq!(
            inputs.first(),
            Some(&Input::State(new_state_event(StateValue::Idle, None, "")))
        );
    }

    #[test]
    fn a_full_queue_drops_meter_samples_before_state() {
        let feed = Feed::default();
        let shared = Arc::downgrade(&feed.shared);
        let first = Input::State(new_state_event(StateValue::Recording, None, ""));
        push(&shared, first.clone());
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
        assert_eq!(drained[0].1, first);
    }
}
