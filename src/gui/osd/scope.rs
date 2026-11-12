//! The dot scope in the island's pill: a small matrix of dots, newest column
//! on the right, each column lit outward from the middle row by its level.
//! While transcribing, the history freezes, squeezes into the middle row and
//! a beam sweeps back and forth across it, so recording hands over to
//! transcribing as one object.

use rune::Spring;

use super::model::{Levels, bar_shape};
use super::motion::Track;

pub(super) const COLUMNS: usize = 22;
pub(super) const ROWS: usize = 5;
const MIDDLE: usize = ROWS / 2;
/// How much older each column's reading is than the one to its right; the
/// matrix holds about a second and a half.
const COLUMN_PERIOD: f64 = 0.07;
/// The newest column reads the level this long ago, so even it interpolates
/// between two meter samples.
const LEVEL_DELAY: f64 = 0.04;
/// One sweep of the beam across the matrix, in seconds.
const SCAN_PASS: f32 = 0.9;
/// How quickly a column eases up to and down from its reading, per second.
const RISE_RATE: f32 = 40.0;
const FALL_RATE: f32 = 20.0;

const SQUEEZE: Spring = Spring::new(0.45, 0.0);
const SCAN: Spring = Spring::new(0.4, 0.0);

/// One dot: how lit it is and how much it blooms, 0 to 1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Dot {
    pub(super) light: f32,
    pub(super) glow: f32,
}

/// What the scope is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    /// The history scrolls with the voice.
    Live,
    /// Frozen and squeezed, with the beam sweeping.
    Working,
    /// Frozen and still, on its way out.
    Rest,
}

pub(super) struct Scope {
    /// What each column draws, 0 to 1, eased towards its reading.
    columns: [f32; COLUMNS],
    /// When the history stopped scrolling.
    frozen: Option<f64>,
    squeeze: Track,
    scan: Track,
    scan_from: f64,
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            columns: [0.0; COLUMNS],
            frozen: None,
            squeeze: Track::at(0.0),
            scan: Track::at(0.0),
            scan_from: 0.0,
        }
    }
}

/// How lit each row of a column is for a shaped level, middle row
/// brightest. Silence leaves a faint middle line; `squeeze` (0 to 1) pulls
/// the light into the middle row.
pub(super) fn column_light(shaped: f32, squeeze: f32) -> [f32; ROWS] {
    let n = shaped.clamp(0.0, 1.0);
    let reach = n * (MIDDLE as f32 + 1.4) * (1.0 - 0.85 * squeeze);
    let intensity = (0.42 + 0.58 * n) * (1.0 - 0.3 * squeeze);
    std::array::from_fn(|row| {
        let distance = row.abs_diff(MIDDLE) as f32;
        let lit = (reach - distance + 0.25).clamp(0.0, 1.0);
        let lit = if distance == 0.0 { lit.max(0.3) } else { lit };
        lit * intensity
    })
}

impl Scope {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Advances one frame; returns whether anything still moves.
    pub(super) fn step(&mut self, levels: &Levels, phase: Phase, now: f64, dt: f32) -> bool {
        match phase {
            Phase::Live => {
                if self.frozen.is_some() {
                    self.reset();
                }
                self.squeeze.target = 0.0;
                self.scan.target = 0.0;
            }
            Phase::Working => {
                self.frozen.get_or_insert(now);
                self.squeeze.target = 1.0;
                if self.scan.target == 0.0 {
                    self.scan_from = now;
                }
                self.scan.target = 1.0;
            }
            Phase::Rest => {
                self.frozen.get_or_insert(now);
                self.scan.target = 0.0;
            }
        }
        let read_at = self.frozen.unwrap_or(now) - LEVEL_DELAY;
        let mut moving = false;
        for (index, column) in self.columns.iter_mut().enumerate() {
            let age = (COLUMNS - 1 - index) as f64 * COLUMN_PERIOD;
            let target = bar_shape(levels.at(read_at - age));
            let rate = if target > *column {
                RISE_RATE
            } else {
                FALL_RATE
            };
            *column += (target - *column) * (1.0 - (-rate * dt).exp());
            moving |= (target - *column).abs() > 0.002;
        }
        self.squeeze.step(SQUEEZE, dt);
        self.scan.step(SCAN, dt);
        moving || self.scan.x > 0.0 || !self.squeeze.settled(1e-3)
    }

    /// The beam's column and its heading (1 rightward, -1 leftward).
    fn beam(&self, now: f64) -> (f32, f32) {
        let t = (now - self.scan_from) as f32 / SCAN_PASS;
        let angle = std::f32::consts::PI * t;
        let position = (0.5 - 0.5 * angle.cos()) * (COLUMNS - 1) as f32;
        (position, if angle.sin() >= 0.0 { 1.0 } else { -1.0 })
    }

    /// Every dot, column by column from the left.
    pub(super) fn dots(&self, now: f64) -> [[Dot; ROWS]; COLUMNS] {
        let squeeze = self.squeeze.x.clamp(0.0, 1.0);
        let scan = self.scan.x.clamp(0.0, 1.0);
        let (beam, heading) = self.beam(now);
        std::array::from_fn(|column| {
            let light = column_light(self.columns[column], squeeze);
            let newest = match COLUMNS - 1 - column {
                0 => 0.9,
                1 => 0.3,
                _ => 0.0,
            } * (1.0 - squeeze);
            let offset = column as f32 - beam;
            let trail = if offset * heading <= 0.0 { 2.4 } else { 0.55 };
            let beam = scan * (-offset.abs() / trail).exp();
            std::array::from_fn(|row| {
                let band = [1.0, 0.68, 0.34][row.abs_diff(MIDDLE)];
                Dot {
                    light: light[row].max(beam * band).clamp(0.0, 1.0),
                    glow: (light[row] * newest)
                        .max(beam * beam * band * 0.9)
                        .clamp(0.0, 1.0),
                }
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f32 = 1.0 / 120.0;

    /// Levels that were quiet until `loud_from`, then loud.
    fn levels(loud_from: f64, until: f64) -> Levels {
        let mut levels = Levels::default();
        let mut t = 0.0;
        while t < until {
            let (rms, peak) = if t < loud_from {
                (0.001, 0.002)
            } else {
                (0.1, 0.5)
            };
            levels.push(super::super::model::meter_level(rms, peak), t);
            t += 1.0 / 30.0;
        }
        levels
    }

    fn run(scope: &mut Scope, levels: &Levels, phase: Phase, from: f64, seconds: f64) -> f64 {
        let mut t = from;
        while t < from + seconds {
            scope.step(levels, phase, t, FRAME);
            t += f64::from(FRAME);
        }
        t
    }

    #[test]
    fn columns_light_outward_from_the_middle_row() {
        let quiet = column_light(0.0, 0.0);
        assert!(quiet[MIDDLE] > 0.0);
        assert_eq!((quiet[0], quiet[1]), (0.0, 0.0));
        let loud = column_light(0.9, 0.0);
        assert!(loud.iter().all(|light| *light > 0.5), "{loud:?}");
        assert_eq!(loud[0], loud[ROWS - 1]);
        assert_eq!(loud[1], loud[ROWS - 2]);
        let squeezed = column_light(0.9, 1.0);
        assert_eq!((squeezed[0], squeezed[1]), (0.0, 0.0));
        assert!(squeezed[MIDDLE] > 0.0 && squeezed[MIDDLE] < loud[MIDDLE]);
    }

    #[test]
    fn the_newest_column_is_on_the_right_and_blooms() {
        let levels = levels(1.5, 2.0);
        let mut scope = Scope::default();
        let now = run(&mut scope, &levels, Phase::Live, 1.5, 0.45);
        let dots = scope.dots(now);
        assert!(dots[COLUMNS - 1][0].light > 0.2, "{:?}", dots[COLUMNS - 1]);
        assert_eq!(dots[0][0].light, 0.0, "the old, quiet end stays dark");
        assert!(dots[COLUMNS - 1][MIDDLE].glow > dots[COLUMNS - 3][MIDDLE].glow);
    }

    #[test]
    fn transcribing_freezes_squeezes_and_sweeps() {
        let levels = levels(0.0, 3.0);
        let mut scope = Scope::default();
        let now = run(&mut scope, &levels, Phase::Live, 1.0, 1.0);
        let frozen = scope.columns;
        let now = run(&mut scope, &levels, Phase::Working, now, 1.2);
        for (column, before) in scope.columns.iter().zip(frozen) {
            assert!((column - before).abs() < 0.05, "the history holds");
        }
        let dots = scope.dots(now);
        let away_from_beam = |dots: &[[Dot; ROWS]; COLUMNS]| {
            let (beam, _) = scope.beam(now);
            let column = if beam > (COLUMNS / 2) as f32 {
                0
            } else {
                COLUMNS - 1
            };
            dots[column]
        };
        let quiet = away_from_beam(&dots);
        assert!(quiet[0].light < 1e-3, "squeezed off the outer rows");
        assert!(quiet[MIDDLE].light > 0.0);

        let mut seen = [false; 2];
        let mut t = now;
        while t < now + SCAN_PASS as f64 * 2.0 {
            let (beam, heading) = scope.beam(t);
            assert!((0.0..=(COLUMNS - 1) as f32).contains(&beam));
            seen[usize::from(heading > 0.0)] = true;
            t += 0.05;
        }
        assert_eq!(seen, [true, true], "the beam goes both ways");
        let (beam, _) = scope.beam(now);
        let lit = scope.dots(now)[beam.round() as usize][MIDDLE];
        assert!(lit.light > 0.9 && lit.glow > 0.5, "{lit:?}");
    }

    #[test]
    fn a_new_recording_starts_from_a_clean_history() {
        let levels = levels(0.0, 2.0);
        let mut scope = Scope::default();
        let now = run(&mut scope, &levels, Phase::Live, 1.0, 0.5);
        let now = run(&mut scope, &levels, Phase::Working, now, 0.6);
        scope.step(&Levels::default(), Phase::Live, now, FRAME);
        assert!(scope.frozen.is_none());
        assert_eq!(scope.squeeze.x, 0.0);
        assert!(scope.columns.iter().all(|column| *column < 1e-6));
    }
}
