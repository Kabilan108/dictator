//! The island's motion: one shape whose width and height ride springs, and
//! the content inside it, which fades and slides in layers so a state's
//! content leaves while the next one arrives. The dot scope is one object
//! across recording and transcribing: it slides between their layouts on
//! its own spring instead of cross-fading. Stepped once per display frame on
//! closed-form springs, so the motion is the same at any frame rate.

use rune::Spring;

use super::model::{Mode, Model};
use super::placement::Placement;

/// The pause between one content leaving and the next arriving.
const HANDOFF: f64 = 0.1;
/// How far the shape opens towards its size before new content arrives, so
/// content never lands clipped by a shape still too small for it.
const OPENED: f32 = 0.85;
/// Sideways speed an error arrives with, in points a second: a small jolt.
const ERROR_KICK: f32 = 900.0;
/// How far a floating island rises as it appears and sinks as it goes.
const SINK: f32 = 8.0;
/// How close to its dot a leaving floating island gets before it fades.
const CLOSING: f32 = 22.0;

const WIDTH: Spring = Spring::new(0.5, 0.36);
const HEIGHT: Spring = Spring::new(0.44, 0.3);
const WIDTH_AWAY: Spring = Spring::new(0.4, 0.12);
const HEIGHT_AWAY: Spring = Spring::new(0.34, 0.0);
const TINT: Spring = Spring::new(0.3, 0.0);
const CONTENT_IN: Spring = Spring::new(0.42, 0.28);
const CONTENT_OUT: Spring = Spring::new(0.28, 0.0);
const FADE_IN: Spring = Spring::new(0.2, 0.0);
const FADE_OUT: Spring = Spring::new(0.24, 0.0);
const RISE: Spring = Spring::new(0.42, 0.34);
const SCOPE_SLIDE: Spring = WIDTH;

/// A value on a spring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Track {
    pub(super) x: f32,
    pub(super) v: f32,
    pub(super) target: f32,
}

impl Track {
    pub(super) const fn at(x: f32) -> Self {
        Self {
            x,
            v: 0.0,
            target: x,
        }
    }

    pub(super) fn step(&mut self, spring: Spring, dt: f32) {
        if self.settled(1e-3) {
            self.x = self.target;
            self.v = 0.0;
            return;
        }
        (self.x, self.v) = spring.sample(self.x, self.v, self.target, dt);
    }

    pub(super) fn settled(&self, epsilon: f32) -> bool {
        (self.x - self.target).abs() < epsilon && self.v.abs() < epsilon * 10.0
    }
}

/// What a content layer draws. Live readings (levels, the clock) come from
/// the model as it draws; this names only what fixes its size.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Content {
    Recording { provider: Option<String> },
    Transcribing { duration_ms: i64 },
    Pasted { transcript: Option<String> },
    Failed { message: String },
}

impl Content {
    /// Whether this content shows the dot scope.
    pub(super) fn scoped(&self) -> bool {
        matches!(
            self,
            Content::Recording { .. } | Content::Transcribing { .. }
        )
    }

    fn of(mode: &Mode) -> Option<Self> {
        Some(match mode {
            Mode::Recording { provider, .. } => Content::Recording {
                provider: provider.clone(),
            },
            Mode::Transcribing { duration_ms } => Content::Transcribing {
                duration_ms: *duration_ms,
            },
            Mode::Pasted { transcript, .. } => Content::Pasted {
                transcript: transcript.clone(),
            },
            Mode::Failed { message, .. } => Content::Failed {
                message: message.clone(),
            },
            Mode::Hidden | Mode::Discarding { .. } | Mode::Leaving => return None,
        })
    }
}

/// How a content lays out in the island.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Fit {
    pub(super) size: (f32, f32),
    /// Where the dot scope's left edge sits from the island's, for content
    /// that shows it.
    pub(super) scope: Option<f32>,
}

pub(super) struct Layer {
    pub(super) content: Content,
    /// 0 gone, 1 settled in place; overshoots on the way in.
    pub(super) presence: Track,
    /// When it starts arriving.
    starts: f64,
    /// When it started arriving, for content that animates from its arrival.
    pub(super) born: f64,
}

impl Layer {
    /// A layer drawn once, settled, to warm the renderer.
    pub(super) fn settled(content: Content) -> Self {
        Self {
            content,
            presence: Track::at(1.0),
            starts: 0.0,
            born: 0.0,
        }
    }

    pub(super) fn leaving(&self) -> bool {
        self.presence.target == 0.0
    }
}

pub(super) struct Motion {
    placement: Placement,
    pub(super) width: Track,
    pub(super) height: Track,
    /// 0 neutral, 1 the error's danger tint.
    pub(super) tint: Track,
    /// The whole island's opacity: a floating island fades in and out; an
    /// attached one comes out of its edge and stays at 1.
    pub(super) fade: Track,
    /// Points away from the edge past its resting place: a floating island
    /// rises into place and sinks away.
    pub(super) rise: Track,
    /// The dot scope's presence, 0 to 1, like a layer's.
    pub(super) scope: Track,
    /// The dot scope's left edge from the island's centre.
    pub(super) scope_x: Track,
    pub(super) layers: Vec<Layer>,
    tab_until: f64,
}

impl Motion {
    pub(super) fn new(placement: Placement) -> Self {
        let (width, height) = placement.seed();
        let attached = placement.attached();
        Self {
            placement,
            width: Track::at(width),
            height: Track::at(height),
            tint: Track::at(0.0),
            fade: Track::at(if attached { 1.0 } else { 0.0 }),
            rise: Track::at(if attached { 0.0 } else { -SINK }),
            scope: Track::at(0.0),
            scope_x: Track::at(0.0),
            layers: Vec::new(),
            tab_until: 0.0,
        }
    }

    /// Whether the island is back in its edge, or gone, with nothing left to
    /// draw.
    pub(super) fn retracted(&self) -> bool {
        let away = if self.placement.attached() {
            self.height.target == 0.0 && self.height.x < 0.25
        } else {
            self.fade.target == 0.0 && self.fade.x < 0.02
        };
        away && self.layers.is_empty() && self.scope.x < 0.02
    }

    /// Whether the scope is on its way out rather than in.
    pub(super) fn scope_leaving(&self) -> bool {
        self.scope.target == 0.0
    }

    /// Advances one frame. `fit` lays out a content. Returns whether
    /// anything still moves.
    pub(super) fn step(
        &mut self,
        model: &Model,
        now: f64,
        dt: f32,
        fit: impl Fn(&Content) -> Fit,
    ) -> bool {
        let wanted = Content::of(&model.mode);
        let current = self.layers.last().filter(|layer| !layer.leaving());
        if wanted.as_ref() != current.map(|layer| &layer.content) {
            self.change(wanted.clone(), now);
        }
        let fitted = wanted.as_ref().map(&fit);

        let tabbing = wanted.is_some() && now < self.tab_until;
        let target = match fitted {
            Some(_) if tabbing => self.placement.tab(),
            Some(fitted) => fitted.size,
            None => self.placement.seed(),
        };
        self.width.target = target.0;
        self.height.target = target.1;
        self.tint.target = if matches!(wanted, Some(Content::Failed { .. })) {
            1.0
        } else {
            0.0
        };
        let (width, height) = if wanted.is_some() {
            (WIDTH, HEIGHT)
        } else {
            (WIDTH_AWAY, HEIGHT_AWAY)
        };
        self.width.step(width, dt);
        self.height.step(height, dt);
        self.height.x = self.height.x.max(0.0);
        self.tint.step(TINT, dt);
        self.step_float(wanted.is_some(), dt);

        let opened = self.width.x >= self.width.target * OPENED
            && self.height.x >= self.height.target * OPENED;
        for layer in &mut self.layers {
            let arriving = layer.presence.x > 0.0 || opened;
            if now >= layer.starts && (layer.leaving() || arriving) {
                if layer.presence.x == 0.0 && !layer.leaving() {
                    layer.born = now;
                }
                let spring = if layer.leaving() {
                    CONTENT_OUT
                } else {
                    CONTENT_IN
                };
                layer.presence.step(spring, dt);
            }
        }
        self.layers
            .retain(|layer| !(layer.leaving() && layer.presence.x < 0.02));
        self.step_scope(wanted.as_ref(), fitted, dt);

        tabbing
            || !self.width.settled(0.05)
            || !self.height.settled(0.05)
            || !self.tint.settled(1e-3)
            || !self.fade.settled(1e-3)
            || !self.rise.settled(0.05)
            || !self.scope.settled(1e-3)
            || !self.scope_x.settled(0.05)
            || self
                .layers
                .iter()
                .any(|layer| !layer.presence.settled(1e-3))
    }

    /// A floating island fades in as it rises, and fades as it sinks once
    /// it has closed back into its dot.
    fn step_float(&mut self, shown: bool, dt: f32) {
        if self.placement.attached() {
            return;
        }
        let closed = self.width.x < self.placement.seed().0 + CLOSING;
        if shown {
            self.fade.target = 1.0;
            self.rise.target = 0.0;
        } else if closed {
            self.fade.target = 0.0;
            self.rise.target = -SINK;
        }
        let fade = if shown { FADE_IN } else { FADE_OUT };
        self.fade.step(fade, dt);
        self.rise.step(RISE, dt);
    }

    /// The scope arrives with the first content that shows it, slides
    /// between the slots of the contents after it, and leaves with the
    /// first that doesn't.
    fn step_scope(&mut self, wanted: Option<&Content>, fitted: Option<Fit>, dt: f32) {
        let slot = fitted.and_then(|fit| Some(fit.scope? - fit.size.0 / 2.0));
        match (wanted, slot) {
            (Some(content), Some(slot)) if content.scoped() => {
                let arriving = self
                    .layers
                    .last()
                    .is_some_and(|layer| !layer.leaving() && layer.presence.x > 0.0);
                if self.scope.x <= 0.0 {
                    self.scope_x = Track::at(slot);
                    if arriving {
                        self.scope.target = 1.0;
                    }
                } else {
                    self.scope_x.target = slot;
                    self.scope.target = 1.0;
                }
            }
            _ => self.scope.target = 0.0,
        }
        let spring = if self.scope_leaving() {
            CONTENT_OUT
        } else {
            CONTENT_IN
        };
        self.scope.step(spring, dt);
        self.scope.x = self.scope.x.max(0.0);
        self.scope_x.step(SCOPE_SLIDE, dt);
    }

    fn change(&mut self, wanted: Option<Content>, now: f64) {
        let showing = self.layers.iter().any(|layer| layer.presence.x > 0.05);
        for layer in &mut self.layers {
            layer.presence.target = 0.0;
        }
        let Some(content) = wanted else { return };
        let mut starts = now + if showing { HANDOFF } else { 0.0 };
        let seed = self.placement.seed();
        if self.height.x < seed.1 + 1.0 {
            let hold = self.placement.tab_hold();
            self.tab_until = now + hold;
            starts = now + hold + 0.08;
        }
        if matches!(content, Content::Failed { .. }) {
            self.width.v += ERROR_KICK;
        }
        self.layers.push(Layer {
            content,
            presence: Track {
                x: 0.0,
                v: 0.0,
                target: 1.0,
            },
            starts,
            born: starts,
        });
    }

    /// Drops all motion, for a reveal that starts from inside the edge or
    /// from a dot.
    pub(super) fn reset(&mut self) {
        *self = Self::new(self.placement);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::osd::model::Input;
    use crate::visual::{StateValue, new_state_event};

    const FRAME: f32 = 1.0 / 120.0;

    const RECORDING_SCOPE: f32 = 50.0;
    const TRANSCRIBING_SCOPE: f32 = 16.0;

    fn fit(content: &Content) -> Fit {
        let (size, scope) = match content {
            Content::Recording { .. } => ((240.0, 40.0), Some(RECORDING_SCOPE)),
            Content::Transcribing { .. } => ((190.0, 40.0), Some(TRANSCRIBING_SCOPE)),
            Content::Pasted { .. } => ((420.0, 64.0), None),
            Content::Failed { .. } => ((360.0, 40.0), None),
        };
        Fit { size, scope }
    }

    /// Runs `seconds` of frames, returning the widest and narrowest widths seen.
    fn run(motion: &mut Motion, model: &Model, from: f64, seconds: f64) -> (f32, f32) {
        let (mut widest, mut narrowest) = (f32::MIN, f32::MAX);
        let mut t = from;
        while t < from + seconds {
            motion.step(model, t, FRAME, fit);
            widest = widest.max(motion.width.x);
            narrowest = narrowest.min(motion.width.x);
            t += f64::from(FRAME);
        }
        (widest, narrowest)
    }

    fn state(model: &mut Model, value: StateValue, at: f64) {
        let duration = std::time::Duration::from_secs(3);
        model.apply(Input::State(new_state_event(value, Some(duration), "")), at);
    }

    #[test]
    fn the_tab_drops_first_then_the_island_overshoots_and_settles() {
        for placement in [Placement::Top, Placement::Bottom] {
            let mut motion = Motion::new(placement);
            let mut model = Model::default();
            state(&mut model, StateValue::Recording, 0.0);
            run(&mut motion, &model, 0.0, 0.09);
            assert!(motion.width.target == placement.tab().0 && motion.height.x > 2.0);
            assert_eq!((motion.fade.x, motion.rise.x), (1.0, 0.0), "no fade");
            let (widest, _) = run(&mut motion, &model, 0.09, 1.5);
            assert!(widest > 244.0, "springs past its width: {widest}");
            assert!((motion.width.x - 240.0).abs() < 0.05);
            assert!(!motion.step(&model, 1.6, FRAME, fit), "settles still");
        }
    }

    #[test]
    fn a_floating_island_rises_from_a_dot_then_springs_open() {
        let mut motion = Motion::new(Placement::Floating);
        let mut model = Model::default();
        state(&mut model, StateValue::Recording, 0.0);
        let dot = Placement::Floating.seed();
        run(&mut motion, &model, 0.0, 0.12);
        assert_eq!((motion.width.x, motion.height.x), dot, "a dot first");
        assert!(motion.fade.x > 0.3 && motion.rise.x > -SINK);
        let mut highest = f32::MIN;
        let mut t = 0.12;
        while t < 1.6 {
            motion.step(&model, t, FRAME, fit);
            highest = highest.max(motion.rise.x);
            t += f64::from(FRAME);
        }
        assert!(highest > 0.3, "rises past its place: {highest}");
        assert!((motion.width.x - 240.0).abs() < 0.05);
        assert!((motion.fade.x - 1.0).abs() < 1e-3 && motion.rise.x.abs() < 0.05);

        state(&mut model, StateValue::Idle, 1.6);
        let mut t = 1.6;
        let mut faded_while_wide = false;
        while !motion.retracted() {
            motion.step(&model, t, FRAME, fit);
            faded_while_wide |= motion.fade.x < 0.9 && motion.width.x > dot.0 + CLOSING;
            t += f64::from(FRAME);
            assert!(t < 3.0, "never left");
        }
        assert!(!faded_while_wide, "closes into its dot before it fades");
        assert!(motion.rise.x < 0.0, "sinks as it goes");
    }

    #[test]
    fn the_scope_slides_between_layouts_without_leaving() {
        let mut motion = Motion::new(Placement::Bottom);
        let mut model = Model::default();
        state(&mut model, StateValue::Recording, 0.0);
        model.apply(
            Input::Meter {
                rms: 0.05,
                peak: 0.3,
            },
            0.5,
        );
        run(&mut motion, &model, 0.0, 1.5);
        assert!((motion.scope.x - 1.0).abs() < 0.01, "{}", motion.scope.x);
        assert!((motion.scope_x.x - (RECORDING_SCOPE - 120.0)).abs() < 0.05);
        state(&mut model, StateValue::Transcribing, 1.5);
        let mut t = 1.5;
        while t < 2.5 {
            motion.step(&model, t, FRAME, fit);
            assert!(motion.scope.x > 0.97, "at {t}: {}", motion.scope.x);
            t += f64::from(FRAME);
        }
        assert!((motion.scope_x.x - (TRANSCRIBING_SCOPE - 95.0)).abs() < 0.05);
        model.apply(
            Input::State(new_state_event(StateValue::Typing, None, "").with_text("done")),
            2.5,
        );
        run(&mut motion, &model, 2.5, 0.6);
        assert!(motion.scope.x < 0.02, "pasting lets it go");
    }

    #[test]
    fn the_first_scope_frame_arrives_with_its_content() {
        let mut motion = Motion::new(Placement::Top);
        let mut model = Model::default();
        state(&mut model, StateValue::Recording, 0.0);
        let mut t = 0.0;
        while t < 1.0 {
            motion.step(&model, t, FRAME, fit);
            let layer = motion.layers[0].presence.x;
            assert!(
                motion.scope.x <= layer + 0.05,
                "at {t}: scope {} ahead of its layer {layer}",
                motion.scope.x
            );
            t += f64::from(FRAME);
        }
    }

    #[test]
    fn content_hands_over_without_two_layers_at_full_strength() {
        let mut motion = Motion::new(Placement::Top);
        let mut model = Model::default();
        state(&mut model, StateValue::Recording, 0.0);
        model.apply(
            Input::Meter {
                rms: 0.05,
                peak: 0.3,
            },
            0.5,
        );
        run(&mut motion, &model, 0.0, 1.0);
        state(&mut model, StateValue::Transcribing, 1.0);
        let mut t = 1.0;
        while t < 2.0 {
            motion.step(&model, t, FRAME, fit);
            let strong = motion
                .layers
                .iter()
                .filter(|layer| layer.presence.x > 0.6)
                .count();
            assert!(strong <= 1, "at {t}");
            t += f64::from(FRAME);
        }
        assert_eq!(motion.layers.len(), 1);
        assert!(matches!(
            motion.layers[0].content,
            Content::Transcribing { .. }
        ));
        let (_, narrowest) = run(&mut motion, &model, 2.0, 0.1);
        assert!((narrowest - 190.0).abs() < 0.1);
    }

    #[test]
    fn leaving_retracts_into_the_edge() {
        let mut motion = Motion::new(Placement::Top);
        let mut model = Model::default();
        state(&mut model, StateValue::Recording, 0.0);
        run(&mut motion, &model, 0.0, 1.0);
        state(&mut model, StateValue::Idle, 1.0);
        run(&mut motion, &model, 1.0, 1.0);
        assert!(motion.retracted());
    }
}
