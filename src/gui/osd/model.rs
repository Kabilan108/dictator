//! What the island shows, decided from the OSD stream alone: a state machine
//! over the daemon's events and the level history the dot scope reads. Times
//! are seconds on the OSD's own clock.

use std::collections::VecDeque;

use crate::visual::StateValue;

/// Meter samples kept: a little over two seconds, more than the scope shows.
const HISTORY: usize = 72;
/// How long a paste stays on screen once it starts.
const PASTED_HOLD: f64 = 1.2;
/// The shortest an error stays, even when the daemon goes idle sooner.
const FAILED_HOLD: f64 = 2.4;
/// The longest an error stays when the daemon never reports idle.
const FAILED_CAP: f64 = 8.0;
/// How long a recording that looked gated may stay transcribing before the
/// island comes back for it.
const DISCARD_GRACE: f64 = 0.45;
/// The daemon's gate (`audio::timeline::gate`): shorter than this, or never
/// louder than -50 dBFS, and the recording is dropped without an upload.
const GATE_MIN_MS: i64 = 300;
const GATE_SILENCE_RMS: f32 = 0.003_162;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Input {
    State {
        value: StateValue,
        duration_ms: Option<i64>,
        message: String,
    },
    Meter {
        rms: f32,
        peak: f32,
    },
    /// `x-transcript`: the text about to be pasted (not sent by the daemon yet).
    Transcript(String),
    /// `x-provider`: the provider the next recording goes to (not sent by the
    /// daemon yet).
    Provider(String),
    Disconnected,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Mode {
    Hidden,
    Recording {
        since: f64,
        base_ms: i64,
        provider: Option<String>,
    },
    /// The recording looked like one the gate drops: the island leaves at
    /// once, and comes back as transcribing if the daemon keeps at it.
    Discarding {
        since: f64,
        duration_ms: i64,
    },
    Transcribing {
        duration_ms: i64,
    },
    Pasted {
        transcript: Option<String>,
        until: f64,
    },
    Failed {
        message: String,
        since: f64,
        idle: bool,
    },
    Leaving,
}

pub(super) struct Model {
    pub(super) mode: Mode,
    daemon: StateValue,
    /// The last `x-provider`, for the recording that starts next.
    provider: Option<String>,
    /// The last `x-transcript`, for the paste that starts next.
    transcript: Option<String>,
    loudest: f32,
    pub(super) levels: Levels,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            mode: Mode::Hidden,
            daemon: StateValue::Idle,
            provider: None,
            transcript: None,
            loudest: 0.0,
            levels: Levels::default(),
        }
    }
}

impl Model {
    /// Whether the island should be out of the edge.
    pub(super) fn present(&self) -> bool {
        matches!(
            self.mode,
            Mode::Recording { .. }
                | Mode::Transcribing { .. }
                | Mode::Pasted { .. }
                | Mode::Failed { .. }
        )
    }

    pub(super) fn recording(&self) -> bool {
        matches!(self.mode, Mode::Recording { .. })
    }

    /// The elapsed time a recording shows at `now`.
    pub(super) fn elapsed_ms(&self, now: f64) -> i64 {
        match self.mode {
            Mode::Recording { since, base_ms, .. } => {
                base_ms + ((now - since).max(0.0) * 1000.0) as i64
            }
            Mode::Transcribing { duration_ms } | Mode::Discarding { duration_ms, .. } => {
                duration_ms
            }
            _ => 0,
        }
    }

    pub(super) fn apply(&mut self, input: Input, at: f64) {
        match input {
            Input::Provider(name) => self.provider = Some(name),
            Input::Transcript(text) => self.transcript = Some(text),
            Input::Meter { rms, peak } => {
                if self.recording() {
                    self.loudest = self.loudest.max(rms);
                    self.levels.push(meter_level(rms, peak), at);
                }
            }
            Input::Disconnected => {
                self.daemon = StateValue::Idle;
                self.transcript = None;
                if self.mode != Mode::Hidden {
                    self.mode = Mode::Leaving;
                }
            }
            Input::State {
                value,
                duration_ms,
                message,
            } => self.state(value, duration_ms, message, at),
        }
    }

    fn state(&mut self, value: StateValue, duration_ms: Option<i64>, message: String, at: f64) {
        self.daemon = value;
        match value {
            StateValue::Recording => {
                if !self.recording() {
                    self.mode = Mode::Recording {
                        since: at,
                        base_ms: duration_ms.unwrap_or(0).max(0),
                        provider: self.provider.take(),
                    };
                    self.loudest = 0.0;
                    self.levels.reset();
                }
            }
            StateValue::Transcribing => {
                let was_recording = self.recording();
                let duration_ms = duration_ms.unwrap_or_else(|| self.elapsed_ms(at)).max(0);
                if matches!(self.mode, Mode::Transcribing { .. }) {
                    return;
                }
                self.mode = if was_recording && likely_discarded(duration_ms, self.loudest) {
                    Mode::Discarding {
                        since: at,
                        duration_ms,
                    }
                } else {
                    Mode::Transcribing { duration_ms }
                };
            }
            StateValue::Typing => {
                if !matches!(self.mode, Mode::Pasted { .. }) {
                    self.mode = Mode::Pasted {
                        transcript: self.transcript.take(),
                        until: at + PASTED_HOLD,
                    };
                }
            }
            StateValue::Error => {
                let message = if message.trim().is_empty() {
                    "Dictation failed".to_string()
                } else {
                    message
                };
                self.mode = Mode::Failed {
                    message,
                    since: at,
                    idle: false,
                };
            }
            StateValue::Idle => {
                self.transcript = None;
                match &mut self.mode {
                    Mode::Hidden | Mode::Pasted { .. } => {}
                    Mode::Failed { idle, .. } => *idle = true,
                    _ => self.mode = Mode::Leaving,
                }
            }
        }
    }

    /// Fires whatever deadline `now` has reached.
    pub(super) fn tick(&mut self, now: f64) {
        let next = match &self.mode {
            Mode::Pasted { until, .. } if now >= *until && self.daemon != StateValue::Typing => {
                Some(Mode::Leaving)
            }
            Mode::Failed { since, idle, .. }
                if (*idle && now >= since + FAILED_HOLD) || now >= since + FAILED_CAP =>
            {
                Some(Mode::Leaving)
            }
            Mode::Discarding { since, duration_ms }
                if now >= since + DISCARD_GRACE && self.daemon == StateValue::Transcribing =>
            {
                Some(Mode::Transcribing {
                    duration_ms: *duration_ms,
                })
            }
            _ => None,
        };
        if let Some(mode) = next {
            self.mode = mode;
        }
    }

    /// When [`Self::tick`] next has something to do.
    pub(super) fn deadline(&self) -> Option<f64> {
        match &self.mode {
            Mode::Pasted { until, .. } => Some(*until),
            Mode::Failed { since, idle, .. } => {
                Some(since + if *idle { FAILED_HOLD } else { FAILED_CAP })
            }
            Mode::Discarding { since, .. } => Some(since + DISCARD_GRACE),
            _ => None,
        }
    }

    /// The island has gone back into the edge.
    pub(super) fn retracted(&mut self) {
        if self.mode == Mode::Leaving {
            self.mode = Mode::Hidden;
        }
    }
}

fn likely_discarded(duration_ms: i64, loudest_rms: f32) -> bool {
    duration_ms < GATE_MIN_MS || loudest_rms < GATE_SILENCE_RMS
}

/// A meter sample as a 0 to 0.92 level: dBFS for the body of speech, the
/// peak for transients. The mapping the shell's dictation pill was tuned on.
pub(super) fn meter_level(rms: f32, peak: f32) -> f32 {
    if !rms.is_finite() || !peak.is_finite() {
        return 0.0;
    }
    let decibels = 20.0 * rms.max(1e-8).log10();
    let body = ((decibels + 50.0) / 38.0).clamp(0.0, 1.0).powf(0.65);
    let transient = ((peak - 0.03) / 0.77).clamp(0.0, 1.0).sqrt();
    (body * 0.88).max(transient * 0.55).clamp(0.0, 0.92)
}

/// Quick to rise, slower to fall, so syllables read as words.
fn follow(previous: f32, target: f32) -> f32 {
    if target > previous {
        previous * 0.45 + target * 0.55
    } else {
        previous * 0.75 + target * 0.25
    }
}

/// How much of a column a level lights, 0 to 1: quiet room noise stays flat.
pub(super) fn bar_shape(level: f32) -> f32 {
    ((level - 0.2) / 0.8).clamp(0.0, 1.0).powf(1.35)
}

#[derive(Default)]
pub(super) struct Levels {
    /// Smoothed levels with the time each arrived.
    samples: VecDeque<(f64, f32)>,
    smoothed: f32,
}

impl Levels {
    fn reset(&mut self) {
        self.samples.clear();
        self.smoothed = 0.0;
    }

    pub(super) fn push(&mut self, level: f32, at: f64) {
        self.smoothed = follow(self.smoothed, level);
        if self.samples.len() == HISTORY {
            self.samples.pop_front();
        }
        self.samples.push_back((at, self.smoothed));
    }

    /// The smoothed level at `t`, interpolated between the samples around it.
    pub(super) fn at(&self, t: f64) -> f32 {
        let Some(&(last_at, last)) = self.samples.back() else {
            return 0.0;
        };
        if t >= last_at {
            return last;
        }
        let mut later = (last_at, last);
        for &(at, level) in self.samples.iter().rev().skip(1) {
            if at <= t {
                let span = (later.0 - at).max(1e-6);
                let share = ((t - at) / span) as f32;
                return level + (later.1 - level) * share;
            }
            later = (at, level);
        }
        later.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(value: StateValue, duration_ms: Option<i64>) -> Input {
        Input::State {
            value,
            duration_ms,
            message: String::new(),
        }
    }

    fn speak(model: &mut Model, from: f64, seconds: f64) {
        let mut t = from;
        while t < from + seconds {
            model.apply(
                Input::Meter {
                    rms: 0.05,
                    peak: 0.3,
                },
                t,
            );
            t += 1.0 / 30.0;
        }
    }

    #[test]
    fn meter_level_maps_silence_to_zero_and_speech_up_to_a_ceiling() {
        assert_eq!(meter_level(0.0, 0.0), 0.0);
        assert_eq!(meter_level(0.003, 0.01), 0.0);
        assert!(meter_level(0.05, 0.3) > 0.6);
        assert_eq!(meter_level(1.0, 1.0), 0.88);
        assert_eq!(meter_level(f32::NAN, 0.5), 0.0);
        assert_eq!(bar_shape(0.1), 0.0);
        assert!((bar_shape(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_dictation_runs_recording_transcribing_pasted_then_leaves() {
        let mut model = Model::default();
        model.apply(Input::Provider("openai".into()), 0.0);
        model.apply(state(StateValue::Recording, Some(0)), 0.1);
        assert!(matches!(
            &model.mode,
            Mode::Recording { provider: Some(name), .. } if name == "openai"
        ));
        speak(&mut model, 0.1, 3.0);
        assert_eq!(model.elapsed_ms(2.1), 2000);
        model.apply(state(StateValue::Transcribing, Some(3000)), 3.1);
        assert_eq!(model.mode, Mode::Transcribing { duration_ms: 3000 });
        model.apply(Input::Transcript("Ship it.".into()), 4.0);
        model.apply(state(StateValue::Typing, None), 4.0);
        model.apply(state(StateValue::Idle, None), 4.6);
        assert!(model.present(), "a paste holds past the daemon's idle");
        model.tick(5.1);
        assert!(model.present());
        model.tick(5.21);
        assert_eq!(model.mode, Mode::Leaving);
        model.retracted();
        assert_eq!(model.mode, Mode::Hidden);

        model.apply(state(StateValue::Recording, Some(0)), 7.0);
        assert!(
            matches!(&model.mode, Mode::Recording { provider: None, .. }),
            "a provider names one recording only"
        );
    }

    #[test]
    fn transcript_reaches_the_paste_it_precedes() {
        let mut model = Model::default();
        model.apply(state(StateValue::Typing, None), 0.0);
        assert!(matches!(
            model.mode,
            Mode::Pasted {
                transcript: None,
                ..
            }
        ));
        model.apply(state(StateValue::Idle, None), 0.5);
        model.tick(2.0);
        model.retracted();
        model.apply(Input::Transcript("hello".into()), 3.0);
        model.apply(state(StateValue::Typing, None), 3.0);
        assert!(matches!(&model.mode, Mode::Pasted { transcript: Some(t), .. } if t == "hello"));
    }

    #[test]
    fn a_gated_recording_leaves_quietly_and_comes_back_if_transcribed_after_all() {
        let mut model = Model::default();
        model.apply(state(StateValue::Recording, Some(0)), 0.0);
        model.apply(
            Input::Meter {
                rms: 0.002,
                peak: 0.006,
            },
            0.1,
        );
        model.apply(state(StateValue::Transcribing, Some(250)), 0.25);
        assert!(matches!(model.mode, Mode::Discarding { .. }));
        assert!(!model.present());
        model.apply(state(StateValue::Idle, None), 0.4);
        assert_eq!(model.mode, Mode::Leaving);

        let mut model = Model::default();
        model.apply(state(StateValue::Recording, Some(0)), 0.0);
        model.apply(state(StateValue::Transcribing, Some(200)), 0.2);
        model.tick(0.5);
        assert!(matches!(model.mode, Mode::Discarding { .. }));
        model.tick(0.66);
        assert_eq!(model.mode, Mode::Transcribing { duration_ms: 200 });
    }

    #[test]
    fn a_spoken_recording_is_not_mistaken_for_a_gated_one() {
        let mut model = Model::default();
        model.apply(state(StateValue::Recording, Some(0)), 0.0);
        speak(&mut model, 0.0, 0.5);
        model.apply(state(StateValue::Transcribing, Some(500)), 0.5);
        assert_eq!(model.mode, Mode::Transcribing { duration_ms: 500 });
    }

    #[test]
    fn an_error_holds_after_idle_then_leaves() {
        let mut model = Model::default();
        model.apply(state(StateValue::Recording, Some(0)), 0.0);
        speak(&mut model, 0.0, 1.0);
        model.apply(state(StateValue::Transcribing, Some(1000)), 1.0);
        model.apply(
            Input::State {
                value: StateValue::Error,
                duration_ms: None,
                message: "provider timed out".into(),
            },
            2.0,
        );
        model.apply(state(StateValue::Idle, None), 2.5);
        model.tick(4.3);
        assert!(
            matches!(&model.mode, Mode::Failed { message, .. } if message == "provider timed out")
        );
        assert_eq!(model.deadline(), Some(2.0 + FAILED_HOLD));
        model.tick(2.0 + FAILED_HOLD);
        assert_eq!(model.mode, Mode::Leaving);

        let mut stuck = Model::default();
        stuck.apply(state(StateValue::Error, None), 0.0);
        assert!(
            matches!(&stuck.mode, Mode::Failed { message, .. } if message == "Dictation failed")
        );
        stuck.tick(8.0);
        assert_eq!(stuck.mode, Mode::Leaving);
    }

    #[test]
    fn a_lost_socket_sends_the_island_away() {
        let mut model = Model::default();
        model.apply(state(StateValue::Recording, Some(0)), 0.0);
        model.apply(Input::Disconnected, 1.0);
        assert_eq!(model.mode, Mode::Leaving);
        model.retracted();
        model.apply(state(StateValue::Idle, None), 2.0);
        assert_eq!(model.mode, Mode::Hidden);
    }

    #[test]
    fn a_snapshot_mid_recording_keeps_its_elapsed_time() {
        let mut model = Model::default();
        model.apply(state(StateValue::Recording, Some(4200)), 10.0);
        assert_eq!(model.elapsed_ms(11.0), 5200);
        model.apply(state(StateValue::Recording, Some(5200)), 11.0);
        assert_eq!(model.elapsed_ms(11.0), 5200);
    }

    #[test]
    fn levels_interpolate_between_samples_and_hold_the_newest() {
        let mut levels = Levels::default();
        assert_eq!(levels.at(1.0), 0.0);
        levels.samples.extend([(0.0, 0.0), (1.0, 1.0)]);
        assert!((levels.at(0.25) - 0.25).abs() < 1e-6);
        assert_eq!(levels.at(2.0), 1.0);
        assert_eq!(levels.at(-1.0), 0.0);
    }
}
