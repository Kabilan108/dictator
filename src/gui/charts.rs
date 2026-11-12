//! Drawn data: the live level meter, a recording's waveform and the
//! statistics charts. Marks are Rune shapes or plain rounded boxes, so they
//! stay sharp at any scale and need no texture.

use std::collections::VecDeque;

use rune::{Color, El, LineCap, Shape, Tx, canvas, col, el, row, text};

use crate::ipc::DaemonState;

use super::controls::Paint;
use super::{LEVEL_HISTORY, Ui, logic};

const METER_PITCH: f32 = 6.0;
const METER_BAR: f32 = 3.0;
pub(super) const METER_HEIGHT: f32 = 92.0;
pub(super) const METER_WIDTH: f32 = LEVEL_HISTORY as f32 * METER_PITCH;

/// A plausible speech envelope for `--demo`, where no daemon feeds levels.
pub(super) fn demo_level(t: f32) -> (f32, f32) {
    let syllable = (t * 0.9).sin().abs() * (t * 0.37 + 1.3).sin().powi(2);
    let phrase = ((t * 0.11).sin() * 0.5 + 0.5).powi(2);
    let rms = (0.015 + 0.5 * syllable * phrase).clamp(0.0, 1.0);
    (rms, (rms * 1.6).min(1.0))
}

/// A speech-shaped waveform for `--demo`, where recordings have no audio.
pub(super) fn demo_waveform(seed: i64, buckets: usize) -> Vec<f32> {
    (0..buckets)
        .map(|index| {
            let t = index as f32 * 0.42 + (seed % 97) as f32;
            let (rms, _) = demo_level(t * 2.3);
            (0.08 + rms * 1.9 * (0.6 + 0.4 * (t * 1.7).sin().abs())).min(1.0)
        })
        .collect()
}

/// The app's mark: three bars in a rounded tile that takes the state's tone.
pub(super) fn brand_mark(p: Paint, state: Option<DaemonState>) -> El {
    let (tile, ink) = match state {
        Some(DaemonState::Recording) => (p.k.c.danger.fg, p.k.c.white),
        _ => (p.k.c.bg_contrast, p.k.c.text_contrast),
    };
    let mut shape = Shape::new();
    for (index, half) in [3.0_f32, 6.0, 4.0].into_iter().enumerate() {
        let x = 6.5 + index as f32 * 4.5;
        shape = shape.line([x, 11.0 - half], [x, 11.0 + half], 2.0, LineCap::Round, ink);
    }
    canvas(shape)
        .key("mark")
        .size(22.0, 22.0)
        .radius(6.0)
        .fill(tile)
        .a11y_hidden()
}

/// The live input level as a mirrored strip of bars, newest on the right.
/// Idle, it rests as a row of dots.
pub(super) fn level_meter(
    p: Paint,
    state: Option<DaemonState>,
    history: &VecDeque<(f32, f32)>,
) -> El {
    let recording = state == Some(DaemonState::Recording);
    let missing = LEVEL_HISTORY.saturating_sub(history.len());
    let levels: Vec<f32> = std::iter::repeat_n(0.0, missing)
        .chain(
            history
                .iter()
                .map(|(rms, peak)| (rms * 1.8).max(peak * 0.9)),
        )
        .collect();
    let rest = p.k.c.text_faint.alpha(0.45);
    let ink = match state {
        Some(DaemonState::Transcribing | DaemonState::Typing) => p.muted(),
        _ => p.strong(),
    };
    let middle = METER_HEIGHT / 2.0;
    let mut shape = Shape::new();
    for index in 0..LEVEL_HISTORY {
        let x = index as f32 * METER_PITCH + METER_PITCH / 2.0;
        let live = recording && index >= missing;
        let (half, color) = if live {
            let level = logic::smoothed(&levels, index).sqrt();
            let age = (index - missing) as f32 / (LEVEL_HISTORY - missing).max(1) as f32;
            (
                1.0 + level * (middle - METER_BAR),
                ink.alpha(0.3 + 0.7 * age * age),
            )
        } else {
            (0.01, rest)
        };
        shape = shape.line(
            [x, middle - half],
            [x, middle + half],
            METER_BAR,
            LineCap::Round,
            color,
        );
    }
    canvas(shape)
        .key("meter")
        .size(METER_WIDTH, METER_HEIGHT)
        .a11y_hidden()
}

/// A recording's waveform. Bars before `played` (0 to 1) take the accent;
/// pressing anywhere seeks to that point.
pub(super) fn waveform(
    p: Paint,
    peaks: &[f32],
    played: f32,
    height: f32,
    seek: impl Fn(&mut Ui, f32, &mut Tx) + Clone + 'static,
) -> El {
    let count = peaks.len().max(1);
    let bars = peaks.iter().enumerate().map(|(index, peak)| {
        let at = index as f32 / count as f32;
        let seek = seek.clone();
        let color = if at < played {
            p.strong()
        } else {
            p.k.c.text_faint.alpha(0.55)
        };
        col()
            .key(format!("bar{index}"))
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .h(height)
            .center()
            .on_press_tx(move |ui: &mut Ui, tx: &mut Tx| seek(ui, at, tx))
            .child(
                el().w(2.0)
                    .h(2.0 + peak.clamp(0.0, 1.0) * (height - 2.0))
                    .radius(1.0)
                    .fill(color),
            )
    });
    row()
        .key("waveform")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .clip_soft(0.0)
        .h(height)
        .cursor(rune::Cursor::Pointer)
        .children(bars)
}

/// One column of a [`bars`] chart.
pub(super) struct Bar {
    /// Height as a share of the tallest value, 0 to 1.
    pub(super) fraction: f32,
    pub(super) tip: String,
    /// The value the chart is about: drawn in the accent, not the neutral.
    pub(super) emphasis: bool,
}

/// A column chart: neutral bars on faint grid lines, the reading under the
/// pointer shown as a tooltip and its column lit.
pub(super) fn bars(p: Paint, key: &str, data: Vec<Bar>, height: f32, gap: f32) -> El {
    let grid = (0..3).map(|line| {
        el().key(format!("grid{line}"))
            .pin(
                Some(0.0),
                Some((height * line as f32 / 3.0).round()),
                Some(0.0),
                None,
            )
            .h(p.hair)
            .fill(p.k.c.border_muted)
    });
    let columns = data.into_iter().enumerate().map(|(index, bar)| {
        let filled = bar.fraction > 0.0;
        let color = if bar.emphasis {
            p.strong()
        } else if filled {
            p.k.c.text_muted.alpha(0.55)
        } else {
            p.k.c.border_base
        };
        col()
            .key(format!("col{index}"))
            .grow(1.0)
            .h(height)
            .justify_end()
            .radius(3.0)
            .hover(p.k.c.hover)
            .help(bar.tip)
            .help_above()
            .child(
                el().w_full()
                    .h(if filled {
                        (bar.fraction * height).max(2.0)
                    } else {
                        1.0
                    })
                    .radius(2.0)
                    .fill(color),
            )
    });
    el().key(key).h(height).a11y_hidden().children(grid).child(
        row()
            .key("columns")
            .w_full()
            .h(height)
            .gap(gap)
            .children(columns),
    )
}

/// Labels under a chart: its two ends and what the middle says about it.
pub(super) fn axis(p: Paint, left: &str, middle: &str, right: &str) -> El {
    let label = |value: &str, color: Color| text(value).font(p.numeric(p.k.ty.small)).color(color);
    row()
        .key("axis")
        .pt(8.0)
        .center_y()
        .child(label(left, p.faint()))
        .child(rune::spacer())
        .child(label(middle, p.muted()))
        .child(rune::spacer())
        .child(label(right, p.faint()))
}

/// Latency samples sorted into equal-width bins up to `ceiling` ms; samples
/// past it land in the last bin.
pub(super) fn histogram(samples: &[i64], bins: usize, ceiling: i64) -> Vec<usize> {
    let mut counts = vec![0; bins.max(1)];
    let (len, last) = (counts.len() as f64, counts.len() - 1);
    for sample in samples {
        let share = (*sample).max(0) as f64 / ceiling.max(1) as f64;
        counts[((share * len) as usize).min(last)] += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_bins_samples_and_clamps_outliers() {
        let cases: [(&[i64], usize, i64, &[usize]); 4] = [
            (&[], 4, 100, &[0, 0, 0, 0]),
            (&[0, 24, 25, 99], 4, 100, &[2, 1, 0, 1]),
            (&[100, 5000], 4, 100, &[0, 0, 0, 2]),
            (&[-5, 10], 2, 0, &[1, 1]),
        ];
        for (samples, bins, ceiling, expected) in cases {
            assert_eq!(histogram(samples, bins, ceiling), expected);
        }
    }

    #[test]
    fn demo_levels_stay_in_range() {
        for step in 0..500 {
            let (rms, peak) = demo_level(step as f32 * 0.37);
            assert!((0.0..=1.0).contains(&rms));
            assert!((rms..=1.0).contains(&peak));
        }
    }
}
