//! `dictator-osd`: the dictation island. By default a capsule floating above
//! the bottom edge that opens from a dot when a recording starts, morphs
//! between states on springs and closes back into the dot when the dictation
//! ends; `--placement bottom|top` attaches it to that edge instead, grown out
//! of it. A windowless Rune app with one click-through overlay, fed by the
//! daemon's OSD socket.

mod feed;
mod model;
mod motion;
mod placement;
mod scope;
mod view;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use rune::{Align, AppSpec, OverlayHandle, OverlayInput, OverlayOptions, OverlaySpec, Tx};

use crate::visual::{StateEvent, StateValue, new_state_event};

use super::controls::Paint;
use feed::Feed;
use model::{Input, Mode, Model};
use motion::Motion;
pub use placement::Placement;
use scope::Scope;

const APP_ID: &str = "org.dictator.Osd";
/// A burst of dictations keeps the overlay's GPU surface; an OSD at rest
/// gives it back.
const GPU_IDLE: Duration = Duration::from_secs(120);
/// The longest frame step the springs take, so a stall doesn't fling them.
const MAX_STEP: f32 = 1.0 / 20.0;
/// The longest the island waits for its surface's first frame before it
/// moves anyway.
const FIRST_FRAME_WAIT: f64 = 0.5;

#[derive(Clone, Debug)]
pub struct OsdOptions {
    pub socket: PathBuf,
    /// Dark or light; `None` follows the system appearance.
    pub dark: Option<bool>,
    pub placement: Placement,
}

pub struct Island {
    model: Model,
    motion: Motion,
    scope: Scope,
    placement: Placement,
    dark: Option<bool>,
    feed: Feed,
    overlay: Option<OverlayHandle<Island>>,
    shown: bool,
    /// Since a show, until the surface presents: the presents counted at the
    /// show and when it was asked for. The opening waits for this, so a
    /// surface that takes a while to map still shows it from the start.
    first_frame: Option<(u64, f64)>,
    epoch: Instant,
    /// Seconds since `epoch` at the last poll; what the view animates from.
    now: f64,
    last_poll: Option<f64>,
    /// A headless scene: drawn as set, never stepped.
    frozen: bool,
}

impl Island {
    fn new(dark: Option<bool>, placement: Placement, feed: Feed) -> Self {
        Self {
            model: Model::default(),
            motion: Motion::new(placement),
            scope: Scope::default(),
            placement,
            dark,
            feed,
            overlay: None,
            shown: false,
            first_frame: None,
            epoch: Instant::now(),
            now: 0.0,
            last_poll: None,
            frozen: false,
        }
    }

    fn step(&mut self, now: f64, dt: f32) -> bool {
        // Layout reads only the type scale, which both appearances share.
        let p = Paint::new(true, 1.0);
        let elapsed = self.model.elapsed_ms(now);
        let phase = match self.model.mode {
            Mode::Recording { .. } => scope::Phase::Live,
            Mode::Transcribing { .. } => scope::Phase::Working,
            _ => scope::Phase::Rest,
        };
        let scoping = self.scope.step(&self.model.levels, phase, now, dt);
        let moving = self.motion.step(&self.model, now, dt, |content| {
            view::fit(content, p, elapsed)
        });
        scoping || moving
    }
}

pub fn run(options: OsdOptions) -> Result<()> {
    rune::set_app_id(APP_ID);
    let (dark, placement) = (options.dark, options.placement);
    let mut app = rune::App::new(spec())
        .init(move || demo(dark, placement))
        .windowless();
    let island = Island::new(dark, placement, Feed::start(options.socket));
    let overlay = app.overlay(
        OverlaySpec {
            name: "dictator-osd",
            size: view::SURFACE,
            edge: placement.edge(),
            margin: 0.0,
            align: Align::Center,
            inset: 0.0,
            interactive: false,
            view: view::view,
        }
        .options(OverlayOptions {
            input: OverlayInput::None,
            prewarm: true,
            ..OverlayOptions::default()
        })
        .poll(poll)
        .setup(setup)
        .release_gpu_after(GPU_IDLE),
        island,
    );
    let this = overlay.clone();
    overlay.update(move |island, _| island.overlay = Some(this));
    app.try_run().map_err(|error| anyhow!(error))
}

fn spec() -> AppSpec<Island> {
    AppSpec {
        title: "Dictator OSD",
        size: view::SURFACE,
        min_size: None,
        init: || demo(None, Placement::default()),
        view: view::view,
        scenes: SCENES,
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        setup: None,
        poll: Some(poll),
        on_key: None,
    }
}

fn setup(island: &mut Island, waker: rune::Waker) {
    island.feed.set_waker(waker);
}

fn poll(island: &mut Island, _clock: f64, tx: &mut Tx) -> bool {
    if island.frozen {
        return false;
    }
    tx.instant();
    let now = island.epoch.elapsed().as_secs_f64();
    let mut dt = island
        .last_poll
        .map_or(0.0, |last| (now - last) as f32)
        .clamp(0.0, MAX_STEP);
    island.last_poll = Some(now);
    island.now = now;

    let inputs = island.feed.drain();
    let changed = !inputs.is_empty();
    for (at, input) in inputs {
        let at = at.saturating_duration_since(island.epoch).as_secs_f64();
        island.model.apply(input, at);
    }
    island.model.tick(now);

    let present = island.model.present();
    if present && !island.shown {
        island.motion.reset();
        island.scope.reset();
        if let Some(overlay) = &island.overlay {
            island.first_frame = Some((overlay.surface_stats().totals.presents, now));
            overlay.show();
        }
        island.shown = true;
    }
    if let Some((presents, asked)) = island.first_frame {
        let drawn = island
            .overlay
            .as_ref()
            .is_some_and(|overlay| overlay.surface_stats().totals.presents > presents);
        if drawn || now - asked > FIRST_FRAME_WAIT {
            island.first_frame = None;
            dt = 0.0;
        }
    }
    let moving = island.first_frame.is_some() || island.step(now, dt);
    if !present && island.motion.retracted() {
        if island.shown
            && let Some(overlay) = &island.overlay
        {
            overlay.hide();
        }
        island.shown = false;
        island.model.retracted();
    }

    if island.shown {
        tx.wake_after(Duration::ZERO);
    } else if let Some(due) = island.model.deadline() {
        tx.wake_after(Duration::from_secs_f64((due - now).max(0.0)));
    }
    // A recording redraws every frame for its clock and record light.
    changed || (island.shown && (moving || island.model.recording()))
}

/// A still island for headless snapshots, with no socket.
fn demo(dark: Option<bool>, placement: Placement) -> Island {
    let mut island = Island::new(dark, placement, Feed::default());
    island.frozen = true;
    island
}

type Scene = (&'static str, fn(&mut Island));

const SCENES: &[Scene] = &[
    ("opening", |island| {
        prepare(island);
        record(island, None, 0.0);
        settle(island, island.placement.tab_hold() * 0.7);
    }),
    ("recording", |island| {
        prepare(island);
        record(island, None, 4.2);
        settle(island, 4.2);
    }),
    ("recording-provider", |island| {
        prepare(island);
        record(island, Some("openai"), 12.6);
        settle(island, 12.6);
    }),
    ("transcribing", |island| {
        prepare(island);
        record(island, None, 7.5);
        let duration = Duration::from_millis(7_500);
        state(
            island,
            new_state_event(StateValue::Transcribing, Some(duration), ""),
            7.5,
        );
        settle(island, 8.2);
    }),
    ("pasted", |island| {
        prepare(island);
        paste(
            island,
            "Okay, so the plan for tomorrow: finish the provider switcher, then write the \
             migration notes and send them to the team before lunch.",
        );
        settle(island, 0.9);
    }),
    ("pasted-short", |island| {
        prepare(island);
        paste(island, "Ship the overlay behind a feature flag first.");
        settle(island, 0.9);
    }),
    ("pasted-plain", |island| {
        prepare(island);
        paste(island, "");
        settle(island, 0.9);
    }),
    ("error", |island| {
        prepare(island);
        let failed = new_state_event(StateValue::Error, None, "transcription failed");
        state(island, failed, 0.0);
        settle(island, 1.2);
    }),
];

/// Scenes take `--placement` from the command line; Rune applies `--theme`
/// to headless runs itself.
fn prepare(island: &mut Island) {
    let args: Vec<String> = std::env::args().collect();
    island.placement = args
        .windows(2)
        .find(|pair| pair[0] == "--placement")
        .and_then(|pair| Placement::parse(&pair[1]))
        .unwrap_or_default();
    island.motion = Motion::new(island.placement);
}

fn state(island: &mut Island, event: StateEvent, at: f64) {
    island.model.apply(Input::State(event), at);
}

fn paste(island: &mut Island, text: &str) {
    state(
        island,
        new_state_event(StateValue::Typing, None, "").with_text(text),
        0.0,
    );
}

/// A recording that has been going for `seconds` of speech.
fn record(island: &mut Island, provider: Option<&str>, seconds: f64) {
    let started = new_state_event(StateValue::Recording, Some(Duration::ZERO), "")
        .with_provider(provider.unwrap_or_default());
    state(island, started, 0.0);
    let mut t = 0.0;
    while t < seconds {
        let (rms, peak) = super::charts::demo_level(t as f32 * 7.0);
        island.model.apply(
            Input::Meter {
                rms: rms * 0.25,
                peak,
            },
            t,
        );
        t += 1.0 / 30.0;
    }
}

/// Steps the island at 120 Hz up to `until` and freezes it there.
fn settle(island: &mut Island, until: f64) {
    let start = match island.model.mode {
        Mode::Recording { since, .. } => since,
        _ => 0.0,
    };
    let frame = 1.0 / 120.0;
    let mut t = start;
    while t < until {
        island.model.tick(t);
        island.step(t, frame as f32);
        t += frame;
    }
    island.now = until;
    island.shown = true;
    island.frozen = true;
}
