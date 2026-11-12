//! `dictator-osd`: the dictation island. A shape on the bottom (or top) edge
//! of the screen that grows out of it when a recording starts, morphs
//! between states on springs and goes back into the edge when the dictation
//! ends; or a capsule floating above the bottom edge that opens from a dot.
//! A windowless Rune app with one click-through overlay, fed by the daemon's
//! OSD socket.

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

use crate::visual::StateValue;

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
    pub dark: bool,
    pub placement: Placement,
}

pub struct Island {
    model: Model,
    motion: Motion,
    scope: Scope,
    placement: Placement,
    dark: bool,
    feed: Feed,
    overlay: Option<OverlayHandle<Island>>,
    shown: bool,
    /// Since a show, until the surface presents: the presents counted at the
    /// show and when it was asked for. The tab drop waits for this, so a
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
    fn new(dark: bool, placement: Placement, feed: Feed) -> Self {
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
        let p = Paint::new(self.dark, 1.0);
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
        init: || demo(true, Placement::default()),
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
    if island.first_frame.is_none() {
        island.step(now, dt);
    }
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
    island.shown || changed
}

/// A still island for headless snapshots, with no socket.
fn demo(dark: bool, placement: Placement) -> Island {
    let mut island = Island::new(dark, placement, Feed::default());
    island.frozen = true;
    island
}

type Scene = (&'static str, fn(&mut Island));

const SCENES: &[Scene] = &[
    ("tab", |island| {
        prepare(island);
        record(island, None, 0.0);
        settle(island, 0.06);
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
        state(island, StateValue::Transcribing, Some(7_500), "", 7.5);
        settle(island, 8.2);
    }),
    ("pasted", |island| {
        prepare(island);
        island.model.apply(
            Input::Transcript(
                "Okay, so the plan for tomorrow: finish the provider switcher, then write \
                 the migration notes and send them to the team before lunch."
                    .into(),
            ),
            0.0,
        );
        state(island, StateValue::Typing, None, "", 0.0);
        settle(island, 0.9);
    }),
    ("pasted-short", |island| {
        prepare(island);
        island.model.apply(
            Input::Transcript("Ship the overlay behind a feature flag first.".into()),
            0.0,
        );
        state(island, StateValue::Typing, None, "", 0.0);
        settle(island, 0.9);
    }),
    ("pasted-plain", |island| {
        prepare(island);
        state(island, StateValue::Typing, None, "", 0.0);
        settle(island, 0.9);
    }),
    ("error", |island| {
        prepare(island);
        state(
            island,
            StateValue::Error,
            None,
            "transcription failed: provider timed out",
            0.0,
        );
        settle(island, 1.2);
    }),
];

/// Scenes follow `--theme` (which Rune reads for headless runs too) and
/// `--placement` from the command line.
fn prepare(island: &mut Island) {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| pair[1].as_str())
    };
    island.dark = flag("--theme") != Some("light");
    island.placement = flag("--placement")
        .and_then(Placement::parse)
        .unwrap_or_default();
    island.motion = Motion::new(island.placement);
}

fn state(island: &mut Island, value: StateValue, duration_ms: Option<i64>, message: &str, at: f64) {
    island.model.apply(
        Input::State {
            value,
            duration_ms,
            message: message.to_string(),
        },
        at,
    );
}

/// A recording that has been going for `seconds` of speech.
fn record(island: &mut Island, provider: Option<&str>, seconds: f64) {
    if let Some(name) = provider {
        island.model.apply(Input::Provider(name.into()), 0.0);
    }
    state(island, StateValue::Recording, Some(0), "", 0.0);
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
