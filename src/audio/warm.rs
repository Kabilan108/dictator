//! Keeps one capture stream open on the selected microphone while the session
//! is unlocked and awake, so a recording can include audio from before its
//! start command.
//!
//! A supervisor thread owns all slow work (pactl, opening parec). The start
//! path only clones the current stream, and falls back to a cold capture
//! whenever no live warm stream exists.

use std::io::{BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use super::capture::{
    CaptureBackend, CaptureStream, LevelState, Placement, StreamOptions, SubscribeCommand,
    on_capture_thread,
};
use super::devices::Fingerprint;

/// Coalesces bursts of device events into one re-resolution.
const RESOLVE_DEBOUNCE: Duration = Duration::from_millis(300);
/// How soon to check again when the server could not say where a recording's
/// stream records from.
const RECHECK_UNPLACED: Duration = Duration::from_secs(1);
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
    /// The stream of the recording in progress, which may be a cold stream
    /// or a warm stream since replaced.
    recording: Mutex<Weak<CaptureStream>>,
    status: Mutex<WarmStatus>,
    /// Set before a hold is sent, so a refresh in progress stops at its next
    /// lookup instead of keeping `mic release` waiting behind all of them.
    hold_requested: AtomicBool,
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
                recording: Mutex::new(Weak::new()),
                status: Mutex::new(WarmStatus::Off),
                hold_requested: AtomicBool::new(false),
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
        self.shared.hold_requested.store(false, Ordering::SeqCst);
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
        self.shared.hold_requested.store(true, Ordering::SeqCst);
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

    /// Includes a recording's stream in the source checks the supervisor runs
    /// when inputs change, until the recording lets go of it. Also schedules
    /// one check, so a move after the start path's own check is not missed.
    pub fn watch_recording(&self, stream: &Arc<CaptureStream>) {
        *self.shared.recording.lock().unwrap() = Arc::downgrade(stream);
        self.resolve_soon();
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
                self.shared.hold_requested.store(false, Ordering::SeqCst);
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
                let at = now + RESOLVE_DEBOUNCE;
                self.resolve_at = Some(self.resolve_at.map_or(at, |pending| pending.min(at)));
            }
            Event::Retry => {
                // A stream that just opened has no audio yet but is not broken.
                let current = self.shared.current.lock().unwrap().clone();
                if current.is_none_or(|warm| warm.stream.is_stalled(now, STALL_GRACE)) {
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
        let (stalled, live) = match self.shared.current.lock().unwrap().as_ref() {
            Some(warm) => (
                warm.stream.is_stalled(now, STALL_GRACE),
                warm.stream.is_live(now),
            ),
            None => (false, false),
        };
        if stalled {
            self.replace(None);
            self.fail("warm microphone stream stopped delivering audio".into());
        } else if live {
            // Backoff resets only once a stream proves it delivers audio, so a
            // parec that starts and dies at once keeps backing off.
            self.failures = 0;
        }
    }

    /// Resolves the microphone and opens or swaps the stream to match.
    fn refresh(&mut self) {
        if self.close_if_moved() {
            self.resolve_at = Some(Instant::now() + RECHECK_UNPLACED);
        }
        if self.hold_requested() {
            return;
        }
        let resolution = (self.backend.resolve)();
        if self.hold_requested() {
            return;
        }
        let (microphone, fingerprint) = match resolution {
            Ok(resolution) => (resolution.microphone, resolution.fingerprint),
            Err(err) => {
                let working = self
                    .shared
                    .current
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|warm| !warm.stream.is_stalled(Instant::now(), STALL_GRACE));
                if working {
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

    /// Ends the warm stream and the recording's stream if the sound server
    /// reports either on a source other than the one it was opened on. A
    /// stream the server cannot place is kept: the start path does not use
    /// it. Returns true when a recording's stream could not be placed, so the
    /// caller checks again soon instead of waiting for the next device event.
    fn close_if_moved(&mut self) -> bool {
        let current = self.shared.current.lock().unwrap().clone();
        let recording = self
            .shared
            .recording
            .lock()
            .unwrap()
            .upgrade()
            .filter(|stream| stream.shared().timeline.lock().unwrap().has_session());
        let is_recording = |stream: &Arc<CaptureStream>| {
            recording.as_ref().is_some_and(|r| Arc::ptr_eq(r, stream))
        };
        let mut unplaced = None;
        if let Some(warm) = &current {
            match warm.stream.check_source(&self.backend.locate) {
                Placement::Moved(_) => {
                    self.replace(None);
                }
                Placement::Unknown(err) if is_recording(&warm.stream) => unplaced = Some(err),
                _ => {}
            }
        }
        if let Some(stream) = &recording
            && !self.hold_requested()
            && !current
                .as_ref()
                .is_some_and(|warm| Arc::ptr_eq(&warm.stream, stream))
            && let Placement::Unknown(err) = stream.check_source(&self.backend.locate)
        {
            unplaced = Some(err);
        }
        if let Some(err) = &unplaced {
            warn!(
                err,
                "could not confirm the recording's audio source; checking again"
            );
        }
        unplaced.is_some()
    }

    /// A hold is on its way; the run loop should take it before more lookups.
    fn hold_requested(&self) -> bool {
        self.shared.hold_requested.load(Ordering::SeqCst)
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
    use crate::audio::capture::Locator;
    use crate::audio::capture::tests::{
        LIVE_SILENCE, microphone, moved_to, on_own_source, options, quiet_level, shell_capture,
        wait_for,
    };
    use crate::audio::devices::{Resolution, ResolvedMicrophone, StreamSource};

    struct Fake {
        selected: Arc<Mutex<Result<ResolvedMicrophone, String>>>,
        resolves: Arc<AtomicUsize>,
        /// What the fake sound server says about where streams record from.
        server: Arc<Mutex<Locator>>,
        backend: CaptureBackend,
    }

    fn fake(script: &'static str) -> Fake {
        let selected = Arc::new(Mutex::new(Ok(microphone("laptop"))));
        let resolves = Arc::new(AtomicUsize::new(0));
        let server = Arc::new(Mutex::new(on_own_source()));
        let resolve_selected = Arc::clone(&selected);
        let resolve_count = Arc::clone(&resolves);
        let locate_server = Arc::clone(&server);
        Fake {
            backend: CaptureBackend {
                resolve: Arc::new(move || {
                    resolve_count.fetch_add(1, Ordering::SeqCst);
                    resolve_selected
                        .lock()
                        .unwrap()
                        .clone()
                        .map(|microphone| Resolution {
                            microphone,
                            fingerprint: None,
                        })
                        .map_err(|err: String| anyhow::anyhow!(err))
                }),
                fingerprint: Arc::new(|| None),
                capture: shell_capture(script),
                subscribe: None,
                locate: Arc::new(move |stream| {
                    let locate = locate_server.lock().unwrap().clone();
                    locate(stream)
                }),
            },
            selected,
            resolves,
            server,
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
    fn release_does_not_wait_behind_a_slow_refresh() {
        const LOOKUP: Duration = Duration::from_millis(1_500);
        let fake = fake(LIVE_SILENCE);
        let slow = Arc::new(AtomicBool::new(false));
        let looking_up = Arc::new(AtomicBool::new(false));
        let resolve = fake.backend.resolve.clone();
        let locate = fake.backend.locate.clone();
        let (resolve_slow, locate_slow, locate_started) = (
            Arc::clone(&slow),
            Arc::clone(&slow),
            Arc::clone(&looking_up),
        );
        let backend = CaptureBackend {
            resolve: Arc::new(move || {
                if resolve_slow.load(Ordering::SeqCst) {
                    std::thread::sleep(LOOKUP);
                }
                resolve()
            }),
            locate: Arc::new(move |stream| {
                if locate_slow.load(Ordering::SeqCst) {
                    locate_started.store(true, Ordering::SeqCst);
                    std::thread::sleep(LOOKUP);
                }
                locate(stream)
            }),
            ..fake.backend.clone()
        };
        let warm = WarmMic::new();
        warm.start(backend, options(), quiet_level()).unwrap();
        wait_for(|| live_source(&warm).is_some());

        // A device event starts a refresh whose lookups each take 1.5 s.
        slow.store(true, Ordering::SeqCst);
        warm.resolve_soon();
        wait_for(|| looking_up.load(Ordering::SeqCst));
        let released_at = Instant::now();
        warm.hold(Hold::Released, true);

        // The release waits for the lookup in flight, not the whole refresh.
        assert!(warm.shared.current.lock().unwrap().is_none());
        assert!(
            released_at.elapsed() < LOOKUP + LOOKUP / 2,
            "{:?}",
            released_at.elapsed()
        );
        warm.shutdown();
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
    fn a_stream_moved_to_another_source_is_closed_and_reopened() {
        let fake = fake(LIVE_SILENCE);
        let warm = started(&fake);
        wait_for(|| live_source(&warm).as_deref() == Some("laptop"));
        let recording = warm.acquire(Instant::now()).unwrap();

        // The server moved the stream although the laptop mic is still selected.
        *fake.server.lock().unwrap() = moved_to("bluez_input.headset", true);
        warm.resolve_soon();
        wait_for(|| {
            recording
                .stream
                .shared()
                .timeline
                .lock()
                .unwrap()
                .is_ended()
        });
        assert_eq!(
            recording.stream.shared().timeline.lock().unwrap().error(),
            Some("audio input moved from laptop to bluez_input.headset")
        );

        // A new stream opens on the selected source.
        *fake.server.lock().unwrap() = on_own_source();
        wait_for(|| live_source(&warm).as_deref() == Some("laptop"));
        let reopened = warm.acquire(Instant::now()).unwrap();
        assert!(!Arc::ptr_eq(&recording.stream, &reopened.stream));
        warm.shutdown();
    }

    /// Takes the live warm stream for a recording, as `Recorder::start` does.
    fn record(warm: &WarmMic) -> WarmStream {
        let recording = warm.acquire(Instant::now()).unwrap();
        begin_recording(warm, &recording.stream);
        recording
    }

    fn begin_recording(warm: &WarmMic, stream: &Arc<CaptureStream>) {
        stream
            .shared()
            .timeline
            .lock()
            .unwrap()
            .begin_session_at_stream_start(usize::MAX);
        warm.watch_recording(stream);
    }

    fn laptop_moved_to_bluetooth(stream: &CaptureStream) -> anyhow::Result<Option<StreamSource>> {
        let source_name = stream.microphone().source_name.clone();
        Ok(Some(if source_name == "laptop" {
            StreamSource {
                source_name: "bluez_input.headset".into(),
                bluetooth: true,
            }
        } else {
            StreamSource {
                source_name,
                bluetooth: false,
            }
        }))
    }

    #[test]
    fn a_recording_stream_is_still_checked_after_a_swap() {
        let fake = fake(LIVE_SILENCE);
        let warm = started(&fake);
        wait_for(|| live_source(&warm).as_deref() == Some("laptop"));
        let recording = record(&warm);

        // A preferred microphone appears during the recording.
        *fake.selected.lock().unwrap() = Ok(microphone("usb"));
        warm.resolve_soon();
        wait_for(|| live_source(&warm).as_deref() == Some("usb"));
        let usb = warm.acquire(Instant::now()).unwrap();

        // The laptop mic goes away and the server moves the recording's stream.
        *fake.server.lock().unwrap() = Arc::new(laptop_moved_to_bluetooth);
        warm.resolve_soon();
        wait_for(|| {
            recording
                .stream
                .shared()
                .timeline
                .lock()
                .unwrap()
                .is_ended()
        });
        assert_eq!(
            recording.stream.shared().timeline.lock().unwrap().error(),
            Some("audio input moved from laptop to bluez_input.headset")
        );
        assert!(Arc::ptr_eq(
            &usb.stream,
            &warm.acquire(Instant::now()).unwrap().stream
        ));
        warm.shutdown();
    }

    #[test]
    fn registering_a_recording_stream_schedules_a_check() {
        let fake = fake(LIVE_SILENCE);
        let warm = started(&fake);
        wait_for(|| live_source(&warm).as_deref() == Some("laptop"));
        let laptop = warm.acquire(Instant::now()).unwrap().stream;
        *fake.selected.lock().unwrap() = Ok(microphone("usb"));
        warm.resolve_soon();
        wait_for(|| live_source(&warm).as_deref() == Some("usb"));

        // The stream moves after the start path checked it but before the
        // recording registers it; no device event follows.
        *fake.server.lock().unwrap() = Arc::new(laptop_moved_to_bluetooth);
        begin_recording(&warm, &laptop);
        wait_for(|| laptop.shared().timeline.lock().unwrap().is_ended());
        assert_eq!(
            laptop.shared().timeline.lock().unwrap().error(),
            Some("audio input moved from laptop to bluez_input.headset")
        );
        warm.shutdown();
    }

    #[test]
    fn a_recording_stream_the_server_cannot_place_is_checked_again() {
        let fake = fake(LIVE_SILENCE);
        let warm = started(&fake);
        wait_for(|| live_source(&warm).as_deref() == Some("laptop"));
        let recording = record(&warm);
        *fake.selected.lock().unwrap() = Ok(microphone("usb"));
        warm.resolve_soon();
        wait_for(|| live_source(&warm).as_deref() == Some("usb"));

        // The recording's stream is moved, but the first lookup of it fails.
        let failed_once = Arc::new(std::sync::atomic::AtomicBool::new(false));
        *fake.server.lock().unwrap() = Arc::new(move |stream| {
            if stream.microphone().source_name == "laptop"
                && !failed_once.swap(true, Ordering::SeqCst)
            {
                anyhow::bail!("pactl failed once");
            }
            laptop_moved_to_bluetooth(stream)
        });
        let before = fake.resolves.load(Ordering::SeqCst);
        warm.resolve_soon();
        wait_for(|| fake.resolves.load(Ordering::SeqCst) > before);
        assert!(
            !recording
                .stream
                .shared()
                .timeline
                .lock()
                .unwrap()
                .is_ended()
        );

        // No device event follows; the supervisor checks again by itself.
        wait_for(|| {
            recording
                .stream
                .shared()
                .timeline
                .lock()
                .unwrap()
                .is_ended()
        });
        assert_eq!(
            recording.stream.shared().timeline.lock().unwrap().error(),
            Some("audio input moved from laptop to bluez_input.headset")
        );
        warm.shutdown();
    }

    #[test]
    fn a_stream_moved_to_bluetooth_is_closed_when_no_other_microphone_remains() {
        let fake = fake(LIVE_SILENCE);
        let warm = started(&fake);
        wait_for(|| live_source(&warm).as_deref() == Some("laptop"));

        // The selected USB mic is unplugged and PulseAudio moves the stream to
        // the Bluetooth default.
        *fake.selected.lock().unwrap() = Err("no non-Bluetooth microphone available".into());
        *fake.server.lock().unwrap() = moved_to("bluez_input.headset", true);
        warm.resolve_soon();
        wait_for(|| {
            warm.status() == WarmStatus::Unavailable("no non-Bluetooth microphone available".into())
        });
        assert!(warm.shared.current.lock().unwrap().is_none());
        assert!(warm.acquire(Instant::now()).is_none());
        warm.shutdown();
    }

    #[test]
    fn a_stream_the_server_cannot_place_survives_a_failed_lookup() {
        let fake = fake(LIVE_SILENCE);
        let warm = started(&fake);
        wait_for(|| live_source(&warm).as_deref() == Some("laptop"));
        let original = warm.acquire(Instant::now()).unwrap();
        let before = fake.resolves.load(Ordering::SeqCst);

        *fake.selected.lock().unwrap() = Err("pactl timed out".into());
        *fake.server.lock().unwrap() = Arc::new(|_| Err(anyhow::anyhow!("pactl timed out")));
        warm.resolve_soon();
        wait_for(|| fake.resolves.load(Ordering::SeqCst) > before);
        assert!(Arc::ptr_eq(
            &original.stream,
            &warm.shared.current.lock().unwrap().as_ref().unwrap().stream
        ));
        assert!(!original.stream.shared().timeline.lock().unwrap().is_ended());
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
    fn capture_that_dies_without_audio_keeps_backing_off() {
        let fake = fake("sleep 0.05; exit 1");
        let opened = Arc::new(Mutex::new(Vec::new()));
        let opened_at = Arc::clone(&opened);
        let resolve = fake.backend.resolve.clone();
        let backend = CaptureBackend {
            resolve: Arc::new(move || {
                opened_at.lock().unwrap().push(Instant::now());
                resolve()
            }),
            ..fake.backend.clone()
        };
        let warm = WarmMic::new();
        warm.start(backend, options(), quiet_level()).unwrap();
        wait_for(|| opened.lock().unwrap().len() >= 3);
        warm.shutdown();
        let opened = opened.lock().unwrap();
        // Retries after one second, then two: the counter did not reset.
        assert!(
            opened[2] - opened[1] >= Duration::from_millis(1_800),
            "{opened:?}"
        );
    }

    #[test]
    fn a_start_before_the_first_chunk_does_not_tear_down_a_new_stream() {
        // Audio begins half a second after the stream opens, and lookups fail
        // after the first, as during a transient pactl problem.
        let fake = fake("sleep 0.5; while :; do head -c 6400 /dev/zero || exit 1; sleep 0.1; done");
        let warm = started(&fake);
        wait_for(|| warm.shared.current.lock().unwrap().is_some());
        *fake.selected.lock().unwrap() = Err("temporary pactl failure".into());

        // Starts during the silent opening fall back to cold starts and must
        // not make the supervisor replace the stream.
        for _ in 0..20 {
            let _ = warm.acquire(Instant::now());
            std::thread::sleep(Duration::from_millis(10));
        }
        wait_for(|| live_source(&warm).as_deref() == Some("laptop"));
        assert_eq!(fake.resolves.load(Ordering::SeqCst), 1);
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
