//! Live meter and state feed from the daemon's OSD socket. The socket already
//! streams 30 Hz level samples and immediate state changes; polling the IPC
//! status endpoint cannot match that cadence.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::ipc::DaemonState;
use crate::visual::{Event, StateValue};

const SAMPLE_CAPACITY: usize = 256;
const RECONNECT_DELAY: Duration = Duration::from_millis(500);

#[derive(Default)]
struct FeedState {
    samples: VecDeque<(f32, f32)>,
    state: Option<DaemonState>,
    state_changed: bool,
}

#[derive(Clone, Default)]
pub struct LevelFeed {
    inner: Arc<Mutex<FeedState>>,
}

#[derive(Debug, Default, PartialEq)]
pub struct FeedUpdate {
    pub samples: Vec<(f32, f32)>,
    pub state: Option<DaemonState>,
    pub state_changed: bool,
}

impl LevelFeed {
    pub fn start(path: PathBuf) -> Self {
        let feed = Self::default();
        let inner = Arc::downgrade(&feed.inner);
        thread::Builder::new()
            .name("dictator-gui-levels".into())
            .spawn(move || {
                loop {
                    let Some(strong) = inner.upgrade() else {
                        return;
                    };
                    match UnixStream::connect(&path) {
                        Ok(stream) => {
                            drop(strong);
                            read_events(stream, &inner);
                        }
                        Err(_) => drop(strong),
                    }
                    thread::sleep(RECONNECT_DELAY);
                }
            })
            .expect("failed to start OSD level feed");
        feed
    }

    pub fn take(&self) -> FeedUpdate {
        let mut inner = self.inner.lock().unwrap();
        FeedUpdate {
            samples: inner.samples.drain(..).collect(),
            state: inner.state,
            state_changed: std::mem::take(&mut inner.state_changed),
        }
    }

    fn apply(inner: &Mutex<FeedState>, event: Event) {
        let mut inner = inner.lock().unwrap();
        apply_event(&mut inner, event);
    }
}

fn read_events(stream: UnixStream, inner: &std::sync::Weak<Mutex<FeedState>>) {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<Event>(&line) else {
            continue;
        };
        let Some(strong) = inner.upgrade() else {
            return;
        };
        LevelFeed::apply(&strong, event);
    }
}

fn apply_event(inner: &mut FeedState, event: Event) {
    match event {
        Event::Meter(meter) => {
            if inner.samples.len() == SAMPLE_CAPACITY {
                inner.samples.pop_front();
            }
            inner
                .samples
                .push_back((meter.rms as f32, meter.peak as f32));
        }
        Event::State(state) => {
            let mapped = map_state(state.value);
            if inner.state != Some(mapped) {
                inner.state = Some(mapped);
                inner.state_changed = true;
            }
            if mapped != DaemonState::Recording {
                inner.samples.clear();
            }
        }
    }
}

fn map_state(value: StateValue) -> DaemonState {
    match value {
        StateValue::Idle => DaemonState::Idle,
        StateValue::Recording => DaemonState::Recording,
        StateValue::Transcribing => DaemonState::Transcribing,
        StateValue::Typing => DaemonState::Typing,
        StateValue::Error => DaemonState::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visual::{new_meter_event, new_state_event};

    #[test]
    fn meter_samples_are_bounded_and_drained() {
        let mut state = FeedState::default();
        for index in 0..(SAMPLE_CAPACITY + 3) {
            apply_event(&mut state, new_meter_event(index as f64, 0.0).into());
        }
        assert_eq!(state.samples.len(), SAMPLE_CAPACITY);
        assert_eq!(state.samples.front().map(|(rms, _)| *rms as usize), Some(3));
    }

    #[test]
    fn state_changes_are_flagged_once_and_clear_samples_when_idle() {
        let feed = LevelFeed::default();
        LevelFeed::apply(&feed.inner, new_meter_event(0.5, 0.9).into());
        LevelFeed::apply(
            &feed.inner,
            new_state_event(StateValue::Recording, None, "").into(),
        );
        let update = feed.take();
        assert_eq!(update.samples, vec![(0.5, 0.9)]);
        assert_eq!(update.state, Some(DaemonState::Recording));
        assert!(update.state_changed);

        LevelFeed::apply(&feed.inner, new_meter_event(0.2, 0.3).into());
        LevelFeed::apply(
            &feed.inner,
            new_state_event(StateValue::Recording, None, "").into(),
        );
        let update = feed.take();
        assert!(!update.state_changed);
        assert_eq!(update.samples.len(), 1);

        LevelFeed::apply(&feed.inner, new_meter_event(0.2, 0.3).into());
        LevelFeed::apply(
            &feed.inner,
            new_state_event(StateValue::Idle, None, "").into(),
        );
        let update = feed.take();
        assert!(update.state_changed);
        assert!(update.samples.is_empty());
    }
}
