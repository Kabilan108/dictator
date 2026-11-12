//! The island's motion: one shape whose width and height ride springs, and
//! the content inside it, which fades and slides in layers so a state's
//! content leaves while the next one arrives. Stepped once per display
//! frame on closed-form springs, so the motion is the same at any frame rate.

use rune::Spring;

use super::model::{Mode, Model};

/// The tab that drops out of the edge before the island opens.
pub(super) const TAB: (f32, f32) = (64.0, 12.0);
/// How long the tab shows before the island springs wide.
const TAB_HOLD: f64 = 0.18;
/// The pause between one content leaving and the next arriving.
const HANDOFF: f64 = 0.1;
/// How far the shape opens towards its size before new content arrives, so
/// content never lands clipped by a shape still too small for it.
const OPENED: f32 = 0.85;
/// Sideways speed an error arrives with, in points a second: a small jolt.
const ERROR_KICK: f32 = 900.0;

const WIDTH: Spring = Spring::new(0.5, 0.36);
const HEIGHT: Spring = Spring::new(0.44, 0.3);
const WIDTH_AWAY: Spring = Spring::new(0.4, 0.12);
const HEIGHT_AWAY: Spring = Spring::new(0.34, 0.0);
const TINT: Spring = Spring::new(0.3, 0.0);
const CONTENT_IN: Spring = Spring::new(0.42, 0.28);
const CONTENT_OUT: Spring = Spring::new(0.28, 0.0);

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

    fn step(&mut self, spring: Spring, dt: f32) {
        if self.settled(1e-3) {
            self.x = self.target;
            self.v = 0.0;
            return;
        }
        (self.x, self.v) = spring.sample(self.x, self.v, self.target, dt);
    }

    fn settled(&self, epsilon: f32) -> bool {
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
    pub(super) width: Track,
    pub(super) height: Track,
    /// 0 neutral, 1 the error's danger tint.
    pub(super) tint: Track,
    pub(super) layers: Vec<Layer>,
    tab_until: f64,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            width: Track::at(TAB.0),
            height: Track::at(0.0),
            tint: Track::at(0.0),
            layers: Vec::new(),
            tab_until: 0.0,
        }
    }
}

impl Motion {
    /// Whether the island is back inside the edge with nothing left to draw.
    pub(super) fn retracted(&self) -> bool {
        self.height.target == 0.0 && self.height.x < 0.25 && self.layers.is_empty()
    }

    /// Advances one frame. `size` gives the island's size for a content.
    /// Returns whether anything still moves.
    pub(super) fn step(
        &mut self,
        model: &Model,
        now: f64,
        dt: f32,
        size: impl Fn(&Content) -> (f32, f32),
    ) -> bool {
        let wanted = Content::of(&model.mode);
        let current = self.layers.last().filter(|layer| !layer.leaving());
        if wanted.as_ref() != current.map(|layer| &layer.content) {
            self.change(wanted.clone(), now);
        }

        let target = match &wanted {
            Some(_) if now < self.tab_until => TAB,
            Some(content) => size(content),
            None => (TAB.0, 0.0),
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

        let tabbing = wanted.is_some() && now < self.tab_until;
        tabbing
            || !self.width.settled(0.05)
            || !self.height.settled(0.05)
            || !self.tint.settled(1e-3)
            || self
                .layers
                .iter()
                .any(|layer| !layer.presence.settled(1e-3))
    }

    fn change(&mut self, wanted: Option<Content>, now: f64) {
        let showing = self.layers.iter().any(|layer| layer.presence.x > 0.05);
        for layer in &mut self.layers {
            layer.presence.target = 0.0;
        }
        let Some(content) = wanted else { return };
        let mut starts = now + if showing { HANDOFF } else { 0.0 };
        if self.height.x < 1.0 {
            self.tab_until = now + TAB_HOLD;
            starts = now + TAB_HOLD + 0.08;
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

    /// Drops all motion, for a reveal that starts from inside the edge.
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::osd::model::Input;
    use crate::visual::StateValue;

    const FRAME: f32 = 1.0 / 120.0;

    fn size(content: &Content) -> (f32, f32) {
        match content {
            Content::Recording { .. } => (240.0, 40.0),
            Content::Transcribing { .. } => (190.0, 40.0),
            Content::Pasted { .. } => (420.0, 64.0),
            Content::Failed { .. } => (360.0, 40.0),
        }
    }

    /// Runs `seconds` of frames, returning the widest and narrowest widths seen.
    fn run(motion: &mut Motion, model: &Model, from: f64, seconds: f64) -> (f32, f32) {
        let (mut widest, mut narrowest) = (f32::MIN, f32::MAX);
        let mut t = from;
        while t < from + seconds {
            motion.step(model, t, FRAME, size);
            widest = widest.max(motion.width.x);
            narrowest = narrowest.min(motion.width.x);
            t += f64::from(FRAME);
        }
        (widest, narrowest)
    }

    fn state(model: &mut Model, value: StateValue, at: f64) {
        model.apply(
            Input::State {
                value,
                duration_ms: Some(3000),
                message: String::new(),
            },
            at,
        );
    }

    #[test]
    fn the_tab_drops_first_then_the_island_overshoots_and_settles() {
        let mut motion = Motion::default();
        let mut model = Model::default();
        state(&mut model, StateValue::Recording, 0.0);
        run(&mut motion, &model, 0.0, 0.09);
        assert!(motion.width.target == TAB.0 && motion.height.x > 2.0);
        let (widest, _) = run(&mut motion, &model, 0.09, 1.5);
        assert!(widest > 244.0, "springs past its width: {widest}");
        assert!((motion.width.x - 240.0).abs() < 0.05);
        assert!(!motion.step(&model, 1.6, FRAME, size), "settles still");
    }

    #[test]
    fn content_hands_over_without_two_layers_at_full_strength() {
        let mut motion = Motion::default();
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
            motion.step(&model, t, FRAME, size);
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
        let mut motion = Motion::default();
        let mut model = Model::default();
        state(&mut model, StateValue::Recording, 0.0);
        run(&mut motion, &model, 0.0, 1.0);
        state(&mut model, StateValue::Idle, 1.0);
        run(&mut motion, &model, 1.0, 1.0);
        assert!(motion.retracted());
    }
}
