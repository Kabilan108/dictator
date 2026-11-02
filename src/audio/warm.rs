//! Keeps one capture stream open on the selected microphone while the session
//! is unlocked and awake, so a recording can include audio from before its
//! start command.
//!
//! A supervisor thread owns all slow work (pactl, opening parec). The start
//! path only clones the current stream, and falls back to a cold capture
//! whenever no live warm stream exists.

use std::io::{BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use super::capture::{
    CaptureBackend, CaptureStream, LevelState, StreamOptions, SubscribeCommand, on_capture_thread,
};
use super::devices::Fingerprint;

/// Coalesces bursts of device events into one re-resolution.
const RESOLVE_DEBOUNCE: Duration = Duration::from_millis(300);
/// How long release and resume wait for the supervisor to act.
const ACK_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the supervisor checks the stream for silence or exit.
const HEALTH_INTERVAL: Duration = Duration::from_secs(2);
/// A stream that opened or delivered audio this long ago is restarted.
const STALL_GRACE: Duration = Duration::from_secs(3);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Why the warm stream is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// `dictator mic release`, typically around a screen locker.
    Released,
    /// logind announced suspend.
    Asleep,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarmStatus {
    Off,
    Opening,
    Open(String),
    Held { released: bool, asleep: bool },
    Unavailable(String),
}

impl std::fmt::Display for WarmStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WarmStatus::Off => f.write_str("off"),
            WarmStatus::Opening => f.write_str("opening"),
            WarmStatus::Open(name) => write!(f, "warm ({name})"),
            WarmStatus::Held { released, asleep } => {
                let reasons = [(*released, "released"), (*asleep, "suspended")]
                    .iter()
                    .filter(|(held, _)| *held)
                    .map(|(_, reason)| *reason)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "closed ({reasons})")
            }
            WarmStatus::Unavailable(error) => write!(f, "unavailable: {error}"),
        }
    }
}

/// A live warm stream and the preferences file version it was resolved from.
#[derive(Clone)]
pub(crate) struct WarmStream {
    pub stream: Arc<CaptureStream>,
    pub fingerprint: Option<Fingerprint>,
    generation: u64,
}

type Ack = Option<mpsc::Sender<()>>;

enum Event {
    Hold(Hold, Ack),
    Unhold(Hold, Ack),
    Resolve,
    Retry,
    Ended(u64),
    Shutdown,
}

struct Shared {
    current: Mutex<Option<WarmStream>>,
    status: Mutex<WarmStatus>,
}

pub(crate) struct WarmMic {
    shared: Arc<Shared>,
    events: Mutex<Option<mpsc::Sender<Event>>>,
    supervisor: Mutex<Option<JoinHandle<()>>>,
}

impl WarmMic {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                current: Mutex::new(None),
                status: Mutex::new(WarmStatus::Off),
            }),
            events: Mutex::new(None),
            supervisor: Mutex::new(None),
        }
    }

    pub fn start(
        &self,
        backend: CaptureBackend,
        options: StreamOptions,
        level: Arc<Mutex<LevelState>>,
    ) -> std::io::Result<()> {
        let mut supervisor = self.supervisor.lock().unwrap();
        if supervisor.is_some() {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel();
        let state = Supervisor {
            backend,
            options,
            level,
            shared: Arc::clone(&self.shared),
            events: tx.clone(),
            released: false,
            asleep: false,
            generation: 0,
            failures: 0,
            retry_at: Some(Instant::now()),
            resolve_at: None,
        };
        *self.shared.status.lock().unwrap() = WarmStatus::Opening;
        *supervisor = Some(
            std::thread::Builder::new()
                .name("dictator-warm-mic".into())
                .spawn(move || state.run(rx))?,
        );
        *self.events.lock().unwrap() = Some(tx);
        Ok(())
    }

    /// Returns the warm stream if it is delivering audio right now. A missing
    /// or silent stream asks the supervisor to retry and returns `None`.
    pub fn acquire(&self, now: Instant) -> Option<WarmStream> {
        let current = self.shared.current.lock().unwrap().clone();
        match current {
            Some(warm) if warm.stream.is_live(now) => Some(warm),
            _ => {
                self.send(Event::Retry);
                None
            }
        }
    }

    /// Closes the warm stream for `hold`. With `wait`, returns once the
    /// supervisor has closed it (bounded by [`ACK_TIMEOUT`]).
    pub fn hold(&self, hold: Hold, wait: bool) {
        self.send_acked(|ack| Event::Hold(hold, ack), wait);
    }

    /// Clears `hold`; the stream reopens once no hold remains.
    pub fn unhold(&self, hold: Hold, wait: bool) {
        self.send_acked(|ack| Event::Unhold(hold, ack), wait);
    }

    fn send_acked(&self, event: impl FnOnce(Ack) -> Event, wait: bool) {
        if !wait {
            self.send(event(None));
            return;
        }
        let (ack_tx, ack_rx) = mpsc::channel();
        self.send(event(Some(ack_tx)));
        // A dropped sender (supervisor not running) also ends the wait.
        let _ = ack_rx.recv_timeout(ACK_TIMEOUT);
    }

    /// Re-resolves the microphone soon and swaps the stream if it changed.
    pub fn resolve_soon(&self) {
        self.send(Event::Resolve);
    }

    pub fn status(&self) -> WarmStatus {
        self.shared.status.lock().unwrap().clone()
    }

    pub fn shutdown(&self) {
        let events = self.events.lock().unwrap().take();
        if let Some(events) = events {
            let _ = events.send(Event::Shutdown);
        }
        let supervisor = self.supervisor.lock().unwrap().take();
        if let Some(supervisor) = supervisor
            && !on_capture_thread()
            && supervisor.join().is_err()
        {
            warn!("warm microphone supervisor panicked");
        }
    }

    fn send(&self, event: Event) {
        if let Some(events) = self.events.lock().unwrap().as_ref() {
            let _ = events.send(event);
        }
    }
}

impl Drop for WarmMic {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Supervisor {
    backend: CaptureBackend,
    options: StreamOptions,
    level: Arc<Mutex<LevelState>>,
    shared: Arc<Shared>,
    events: mpsc::Sender<Event>,
    released: bool,
    asleep: bool,
    generation: u64,
    failures: u32,
    retry_at: Option<Instant>,
    resolve_at: Option<Instant>,
}

impl Supervisor {
    fn run(mut self, rx: mpsc::Receiver<Event>) {
        let watcher = self
            .backend
            .subscribe
            .clone()
            .and_then(|command| Watcher::start(command, self.events.clone()));
        loop {
            let event = if self.is_held() {
                match rx.recv() {
                    Ok(event) => Some(event),
                    Err(_) => break,
                }
            } else {
                let now = Instant::now();
                let wake = [self.retry_at, self.resolve_at, Some(now + HEALTH_INTERVAL)]
                    .into_iter()
                    .flatten()
                    .min()
                    .unwrap_or(now);
                match rx.recv_timeout(wake.saturating_duration_since(now)) {
                    Ok(event) => Some(event),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            };
            match event {
                Some(Event::Shutdown) => break,
                Some(event) => self.handle(event),
                None => {}
            }
            if !self.is_held() {
                self.tick(Instant::now());
            }
        }
        self.replace(None);
        *self.shared.status.lock().unwrap() = WarmStatus::Off;
        if let Some(watcher) = watcher {
            watcher.stop();
        }
    }

    fn is_held(&self) -> bool {
        self.released || self.asleep
    }

    fn handle(&mut self, event: Event) {
        let now = Instant::now();
        match event {
            Event::Hold(hold, ack) => {
                match hold {
                    Hold::Released => self.released = true,
                    Hold::Asleep => self.asleep = true,
                }
                self.retry_at = None;
                self.resolve_at = None;
                if self.replace(None) {
                    info!(reason = ?hold, "warm microphone closed");
                }
                *self.shared.status.lock().unwrap() = WarmStatus::Held {
                    released: self.released,
                    asleep: self.asleep,
                };
                acknowledge(ack);
            }
            Event::Unhold(hold, ack) => {
                match hold {
                    Hold::Released => self.released = false,
                    Hold::Asleep => self.asleep = false,
                }
                if self.is_held() {
                    *self.shared.status.lock().unwrap() = WarmStatus::Held {
                        released: self.released,
                        asleep: self.asleep,
                    };
                } else {
                    info!(reason = ?hold, "warm microphone resuming");
                    *self.shared.status.lock().unwrap() = WarmStatus::Opening;
                    self.failures = 0;
                    self.retry_at = Some(now);
                }
                acknowledge(ack);
            }
            Event::Resolve | Event::Retry if self.is_held() => {}
            Event::Resolve => {
                if self.resolve_at.is_none() {
                    self.resolve_at = Some(now + RESOLVE_DEBOUNCE);
                }
            }
            Event::Retry => {
                let current = self.shared.current.lock().unwrap().clone();
                if current.is_none_or(|warm| !warm.stream.is_live(now)) {
                    self.retry_at = Some(self.retry_at.map_or(now, |at| at.min(now)));
                }
            }
            Event::Ended(generation) => {
                let ended = self
                    .shared
                    .current
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|warm| warm.generation == generation);
                if ended {
                    self.replace(None);
                    self.fail("warm microphone stream ended".into());
                }
            }
            Event::Shutdown => {}
        }
    }

    fn tick(&mut self, now: Instant) {
        let due = |at: Option<Instant>| at.is_some_and(|at| at <= now);
        if due(self.retry_at) || due(self.resolve_at) {
            self.retry_at = None;
            self.resolve_at = None;
            self.refresh();
            return;
        }
        let stalled = self
            .shared
            .current
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|warm| warm.stream.is_stalled(now, STALL_GRACE));
        if stalled {
            self.replace(None);
            self.fail("warm microphone stream stopped delivering audio".into());
        }
    }

    /// Resolves the microphone and opens or swaps the stream to match.
    fn refresh(&mut self) {
        let fingerprint = (self.backend.fingerprint)();
        let microphone = match (self.backend.resolve)() {
            Ok(microphone) => microphone,
            Err(err) => {
                let live = self
                    .shared
                    .current
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|warm| warm.stream.is_live(Instant::now()));
                if live {
                    // Keep a working stream through a transient pactl failure.
                    warn!(err = %format!("{err:#}"), "failed to re-resolve warm microphone");
                    self.schedule_retry();
                } else {
                    self.replace(None);
                    self.fail(format!("{err:#}"));
                }
                return;
            }
        };

        {
            let mut current = self.shared.current.lock().unwrap();
            if let Some(warm) = current.as_mut()
                && warm.stream.microphone().source_name == microphone.source_name
                && !warm.stream.is_stalled(Instant::now(), STALL_GRACE)
            {
                warm.fingerprint = fingerprint;
                self.failures = 0;
                return;
            }
        }

        if self.shared.current.lock().unwrap().is_none() {
            *self.shared.status.lock().unwrap() = WarmStatus::Opening;
        }
        self.generation += 1;
        let generation = self.generation;
        let events = self.events.clone();
        let name = microphone.name.clone();
        match CaptureStream::open(
            &self.backend.capture,
            microphone,
            self.options,
            Arc::clone(&self.level),
            Some(Box::new(move || {
                let _ = events.send(Event::Ended(generation));
            })),
        ) {
            Ok(stream) => {
                self.replace(Some(WarmStream {
                    stream,
                    fingerprint,
                    generation,
                }));
                self.failures = 0;
                *self.shared.status.lock().unwrap() = WarmStatus::Open(name.clone());
                info!(microphone = name, "warm microphone open");
            }
            Err(err) => {
                let open = self.shared.current.lock().unwrap().is_some();
                warn!(err = %format!("{err:#}"), "failed to open warm microphone");
                if open {
                    self.schedule_retry();
                } else {
                    self.fail(format!("{err:#}"));
                }
            }
        }
    }

    fn fail(&mut self, error: String) {
        debug!(err = %error, failures = self.failures + 1, "warm microphone unavailable");
        *self.shared.status.lock().unwrap() = WarmStatus::Unavailable(error);
        self.schedule_retry();
    }

    fn schedule_retry(&mut self) {
        self.failures = self.failures.saturating_add(1);
        let delay = Duration::from_secs(1)
            .saturating_mul(1 << self.failures.saturating_sub(1).min(6))
            .min(MAX_BACKOFF);
        self.retry_at = Some(Instant::now() + delay);
    }

    /// Swaps the current stream. The old stream is dropped outside the lock;
    /// a recording that still holds it keeps capturing until it finishes.
    fn replace(&self, next: Option<WarmStream>) -> bool {
        let previous = std::mem::replace(&mut *self.shared.current.lock().unwrap(), next);
        previous.is_some()
    }
}

fn acknowledge(ack: Ack) {
    if let Some(ack) = ack {
        let _ = ack.send(());
    }
}

/// Runs `pactl subscribe` and turns source and default-source changes into
/// re-resolution events, restarting pactl if the sound server restarts.
struct Watcher {
    stop: Arc<WatcherStop>,
    thread: JoinHandle<()>,
}

#[derive(Default)]
struct WatcherStop {
    state: Mutex<(bool, Option<Child>)>,
    wake: Condvar,
}

impl Watcher {
    fn start(command: SubscribeCommand, events: mpsc::Sender<Event>) -> Option<Self> {
        let stop = Arc::new(WatcherStop::default());
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("dictator-mic-watch".into())
            .spawn(move || watch(&command, &events, &thread_stop))
            .map_err(|err| warn!(err = %err, "failed to start microphone watcher"))
            .ok()?;
        Some(Self { stop, thread })
    }

    fn stop(self) {
        {
            let mut state = self.stop.state.lock().unwrap();
            state.0 = true;
            if let Some(child) = state.1.as_mut() {
                let _ = child.kill();
            }
        }
        self.stop.wake.notify_all();
        if self.thread.join().is_err() {
            warn!("microphone watcher panicked");
        }
    }
}

fn watch(command: &SubscribeCommand, events: &mpsc::Sender<Event>, stop: &WatcherStop) {
    let mut failures = 0u32;
    loop {
        let started = Instant::now();
        let mut process = command();
        process
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        match process.spawn() {
            Ok(mut child) => {
                let stdout = child.stdout.take();
                {
                    let mut state = stop.state.lock().unwrap();
                    if state.0 {
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                    state.1 = Some(child);
                }
                if let Some(stdout) = stdout {
                    for line in BufReader::new(stdout).lines() {
                        let Ok(line) = line else { break };
                        if is_microphone_event(&line) && events.send(Event::Resolve).is_err() {
                            break;
                        }
                    }
                }
                let child = stop.state.lock().unwrap().1.take();
                if let Some(mut child) = child {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            Err(err) => warn!(err = %err, "failed to run pactl subscribe"),
        }
        // The sound server may have restarted with different sources.
        let _ = events.send(Event::Resolve);
        failures = if started.elapsed() > Duration::from_secs(30) {
            0
        } else {
            failures.saturating_add(1)
        };
        let delay = Duration::from_secs(1)
            .saturating_mul(1 << failures.min(5))
            .min(Duration::from_secs(30));
        let state = stop.state.lock().unwrap();
        let (state, _) = stop
            .wake
            .wait_timeout_while(state, delay, |state| !state.0)
            .unwrap();
        if state.0 {
            return;
        }
    }
}

/// Source additions and removals, and server changes (the default source).
fn is_microphone_event(line: &str) -> bool {
    [
        "'new' on source #",
        "'remove' on source #",
        "'change' on server",
    ]
    .iter()
    .any(|pattern| line.contains(pattern))
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::audio::capture::tests::{
        LIVE_SILENCE, microphone, options, quiet_level, shell_capture, wait_for,
    };
    use crate::audio::devices::ResolvedMicrophone;

    struct Fake {
        selected: Arc<Mutex<Result<ResolvedMicrophone, String>>>,
        resolves: Arc<AtomicUsize>,
        backend: CaptureBackend,
    }

    fn fake(script: &'static str) -> Fake {
        let selected = Arc::new(Mutex::new(Ok(microphone("laptop"))));
        let resolves = Arc::new(AtomicUsize::new(0));
        let resolve_selected = Arc::clone(&selected);
        let resolve_count = Arc::clone(&resolves);
        Fake {
            backend: CaptureBackend {
                resolve: Arc::new(move || {
                    resolve_count.fetch_add(1, Ordering::SeqCst);
                    resolve_selected
                        .lock()
                        .unwrap()
                        .clone()
                        .map_err(|err: String| anyhow::anyhow!(err))
                }),
                fingerprint: Arc::new(|| None),
                capture: shell_capture(script),
                subscribe: None,
            },
            selected,
            resolves,
        }
    }

    fn started(fake: &Fake) -> WarmMic {
        let warm = WarmMic::new();
        warm.start(fake.backend.clone(), options(), quiet_level())
            .unwrap();
        warm
    }

    fn live_source(warm: &WarmMic) -> Option<String> {
        warm.acquire(Instant::now())
            .map(|warm| warm.stream.microphone().source_name.clone())
    }

    #[test]
    fn opens_on_start_and_closes_while_released_or_asleep() {
        let fake = fake(LIVE_SILENCE);
        let warm = started(&fake);
        wait_for(|| live_source(&warm).as_deref() == Some("laptop"));
        assert_eq!(warm.status(), WarmStatus::Open("laptop".into()));

        // A waiting release returns with the stream already closed.
        warm.hold(Hold::Released, true);
        assert!(warm.shared.current.lock().unwrap().is_none());
        assert!(warm.acquire(Instant::now()).is_none());

        // Waking while still released keeps the stream closed.
        warm.hold(Hold::Asleep, false);
        warm.unhold(Hold::Asleep, true);
        assert_eq!(
            warm.status(),
            WarmStatus::Held {
                released: true,
                asleep: false,
            }
        );
        std::thread::sleep(Duration::from_millis(300));
        assert!(warm.shared.current.lock().unwrap().is_none());

        warm.unhold(Hold::Released, true);
        assert_ne!(warm.status(), WarmStatus::Off);
        wait_for(|| live_source(&warm).is_some());
        warm.shutdown();
        assert_eq!(warm.status(), WarmStatus::Off);
    }

    #[test]
    fn a_recording_keeps_a_released_stream_until_it_lets_go() {
        let fake = fake(LIVE_SILENCE);
        let warm = started(&fake);
        wait_for(|| live_source(&warm).is_some());
        let held = warm.acquire(Instant::now()).unwrap();

        warm.hold(Hold::Released, true);
        assert!(warm.shared.current.lock().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(300));
        assert!(held.stream.is_live(Instant::now()));

        let shared = Arc::clone(held.stream.shared());
        drop(held);
        assert!(shared.timeline.lock().unwrap().is_ended());
        warm.shutdown();
    }

    #[test]
    fn microphone_change_swaps_the_stream() {
        let fake = fake(LIVE_SILENCE);
        let warm = started(&fake);
        wait_for(|| live_source(&warm).as_deref() == Some("laptop"));
        let before = fake.resolves.load(Ordering::SeqCst);

        // Same selection: re-resolution keeps the existing stream.
        let original = warm.acquire(Instant::now()).unwrap();
        warm.resolve_soon();
        wait_for(|| fake.resolves.load(Ordering::SeqCst) > before);
        assert!(Arc::ptr_eq(
            &original.stream,
            &warm.acquire(Instant::now()).unwrap().stream
        ));
        drop(original);

        *fake.selected.lock().unwrap() = Ok(microphone("usb"));
        warm.resolve_soon();
        wait_for(|| live_source(&warm).as_deref() == Some("usb"));
        warm.shutdown();
    }

    #[test]
    fn dead_stream_is_reopened_with_backoff() {
        let fake = fake("sleep 0.3; head -c 6400 /dev/zero; sleep 0.2; exit 1");
        let warm = started(&fake);
        // The first stream exits after half a second; the supervisor resolves
        // and opens again after a one-second backoff.
        wait_for(|| fake.resolves.load(Ordering::SeqCst) >= 2);
        warm.shutdown();
    }

    #[test]
    fn resolution_failure_is_reported_and_retried() {
        let fake = fake(LIVE_SILENCE);
        *fake.selected.lock().unwrap() = Err("no non-Bluetooth microphone available".into());
        let warm = started(&fake);
        wait_for(|| {
            warm.status() == WarmStatus::Unavailable("no non-Bluetooth microphone available".into())
        });
        assert!(warm.acquire(Instant::now()).is_none());

        *fake.selected.lock().unwrap() = Ok(microphone("usb"));
        // A failed start asks for an immediate retry instead of waiting out the backoff.
        warm.acquire(Instant::now());
        wait_for(|| live_source(&warm).as_deref() == Some("usb"));
        warm.shutdown();
    }

    #[test]
    fn watcher_turns_device_events_into_resolutions_and_stops_cleanly() {
        let (tx, rx) = mpsc::channel();
        let command: SubscribeCommand = Arc::new(|| {
            let mut command = Command::new("/bin/sh");
            command.args([
                "-c",
                "echo \"Event 'new' on client #1\"; echo \"Event 'remove' on source #7\"; exec sleep 30",
            ]);
            command
        });
        let watcher = Watcher::start(command, tx).unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Event::Resolve)
        ));
        let stopped_at = Instant::now();
        watcher.stop();
        assert!(stopped_at.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn subscription_filter_matches_source_and_server_changes() {
        assert!(is_microphone_event("Event 'new' on source #63"));
        assert!(is_microphone_event("Event 'remove' on source #63"));
        assert!(is_microphone_event("Event 'change' on server #-1"));
        assert!(!is_microphone_event("Event 'change' on source #63"));
        assert!(!is_microphone_event("Event 'new' on source-output #12"));
        assert!(!is_microphone_event("Event 'new' on client #14869"));
    }
}
