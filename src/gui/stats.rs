//! The Statistics pane: headline figures, activity over the last month and
//! through the day, and how long transcription takes.

use rune::{El, TextStyle, col, row, text};

use super::backend::Stats;
use super::charts::{self, Bar};
use super::controls::{self, GUTTER, Paint, ROW_PX, SECTION_GAP};
use super::{Ui, icons, logic};

const FIGURE: TextStyle = TextStyle::new(26.0, 600.0).lh(32.0);
const CHART_HEIGHT: f32 = 104.0;
const LATENCY_BINS: usize = 28;

pub(super) fn pane(ui: &Ui, p: Paint) -> (Vec<El>, El) {
    let Some(stats) = &ui.stats else {
        let (title, hint) = if ui.notice.is_empty() {
            ("Loading statistics…", "")
        } else {
            (ui.notice.as_str(), "")
        };
        return (
            Vec::new(),
            controls::empty_state(p, "stats-empty", &icons::CHART, title, hint),
        );
    };
    let page = col()
        .key("stats")
        .px(GUTTER - ROW_PX)
        .pt(4.0)
        .pb(GUTTER)
        .gap(SECTION_GAP)
        .child(tiles(p, stats))
        .child(activity(p, stats))
        .child(
            row()
                .key("pair")
                .gap(SECTION_GAP - 8.0)
                .items_start()
                .child(hours(p, stats).grow(1.0).shrink().w(0.0))
                .child(latency(p, stats).grow(1.0).shrink().w(0.0)),
        )
        .child(models(p, stats));
    (Vec::new(), controls::scroll_fill(p, "stats-scroll", page))
}

fn tiles(p: Paint, stats: &Stats) -> El {
    let week = match stats.last_week_words {
        0 => "no words last week".to_string(),
        last => {
            let change = (stats.this_week_words - last) as f64 / last as f64 * 100.0;
            format!("{change:+.0}% on last week")
        }
    };
    let success = if stats.total > 0 {
        format!(
            "{:.1}% transcribed",
            stats.successful as f64 / stats.total as f64 * 100.0
        )
    } else {
        "none yet".to_string()
    };
    let length = stats
        .median_duration_ms
        .map(|median| format!("median {}", logic::format_duration(median)))
        .unwrap_or_else(|| "no recordings".to_string());
    row()
        .key("tiles")
        .gap(12.0)
        .child(tile(
            p,
            "today",
            &logic::format_count(stats.today_words),
            "Words today",
            &logic::format_hours(stats.today_duration_ms),
        ))
        .child(tile(
            p,
            "week",
            &logic::format_count(stats.this_week_words),
            "Words this week",
            &week,
        ))
        .child(tile(
            p,
            "recordings",
            &logic::format_count(stats.total),
            "Recordings",
            &success,
        ))
        .child(tile(
            p,
            "time",
            &logic::format_hours(stats.duration_ms),
            "Time dictated",
            &length,
        ))
}

fn tile(p: Paint, key: &str, figure: &str, label: &str, note: &str) -> El {
    controls::raised(p, key)
        .grow(1.0)
        .shrink()
        .w(0.0)
        .px(ROW_PX)
        .py(14.0)
        .gap(2.0)
        .label(format!("{label}: {figure}, {note}"))
        .child(
            text(label)
                .font(p.k.ty.small)
                .color(p.muted())
                .a11y_hidden(),
        )
        .child(
            text(figure)
                .font(p.numeric(FIGURE))
                .color(p.strong())
                .a11y_hidden(),
        )
        .child(
            text(note)
                .font(p.numeric(p.k.ty.small))
                .color(p.faint())
                .truncate()
                .a11y_hidden(),
        )
}

fn chart_card(p: Paint, key: &str, title: &str, reading: &str, chart: El, axis: El) -> El {
    let header = row()
        .key("chart-header")
        .gap(8.0)
        .center_y()
        .child(text(title).font(p.k.ty.ui_medium).color(p.text()).grow(1.0))
        .child(text(reading).font(p.numeric(p.k.ty.small)).color(p.muted()));
    controls::raised(p, key)
        .px(ROW_PX)
        .py(14.0)
        .gap(14.0)
        .child(header)
        .child(col().child(chart).child(axis))
}

fn activity(p: Paint, stats: &Stats) -> El {
    let days = &stats.daily_last_30;
    let peak = days.iter().map(|day| day.words).max().unwrap_or(0).max(1);
    let total: i64 = days.iter().map(|day| day.words).sum();
    let last = days.len().saturating_sub(1);
    let bars = days
        .iter()
        .enumerate()
        .map(|(index, day)| Bar {
            fraction: day.words as f32 / peak as f32,
            tip: format!(
                "{} · {} words · {} · {}",
                day.date.format("%a %b %-d"),
                logic::format_count(day.words),
                logic::plural(day.recordings as usize, "recording"),
                logic::format_hours(day.duration_ms)
            ),
            emphasis: index == last,
        })
        .collect();
    let label = |date: Option<chrono::NaiveDate>| {
        date.map(|date| date.format("%b %-d").to_string())
            .unwrap_or_default()
    };
    chart_card(
        p,
        "activity",
        "Words per day",
        &format!("{} in 30 days", logic::format_count(total)),
        charts::bars(p, "activity-bars", bars, CHART_HEIGHT, 4.0),
        charts::axis(
            p,
            &label(days.first().map(|day| day.date)),
            &format!("peak {}", logic::format_count(peak)),
            "today",
        ),
    )
}

fn hours(p: Paint, stats: &Stats) -> El {
    let peak = stats.by_hour.iter().copied().max().unwrap_or(0).max(1);
    let busiest = stats
        .by_hour
        .iter()
        .enumerate()
        .max_by_key(|(_, count)| **count)
        .filter(|(_, count)| **count > 0)
        .map(|(hour, _)| hour);
    let bars = stats
        .by_hour
        .iter()
        .enumerate()
        .map(|(hour, count)| Bar {
            fraction: *count as f32 / peak as f32,
            tip: format!(
                "{hour:02}:00 to {:02}:00 · {}",
                (hour + 1) % 24,
                logic::plural(*count as usize, "recording")
            ),
            emphasis: busiest == Some(hour),
        })
        .collect();
    chart_card(
        p,
        "hours",
        "Time of day",
        &busiest
            .map(|hour| format!("busiest {hour:02}:00"))
            .unwrap_or_default(),
        charts::bars(p, "hour-bars", bars, CHART_HEIGHT, 3.0),
        charts::axis(p, "00:00", "12:00", "23:00"),
    )
}

fn latency(p: Paint, stats: &Stats) -> El {
    let ceiling = stats
        .p99
        .or(stats.max)
        .unwrap_or(0)
        .max(stats.p95.unwrap_or(0))
        .max(1);
    let counts = charts::histogram(&stats.latency_samples, LATENCY_BINS, ceiling);
    let peak = counts.iter().copied().max().unwrap_or(0).max(1);
    let median_bin = stats
        .p50
        .map(|p50| ((p50 as f64 / ceiling as f64) * LATENCY_BINS as f64) as usize)
        .map(|bin| bin.min(LATENCY_BINS - 1));
    let width = ceiling as f64 / LATENCY_BINS as f64;
    let bars = counts
        .iter()
        .enumerate()
        .map(|(bin, count)| Bar {
            fraction: *count as f32 / peak as f32,
            tip: format!(
                "{} to {} · {}",
                logic::format_latency(Some((bin as f64 * width) as i64)),
                logic::format_latency(Some(((bin + 1) as f64 * width) as i64)),
                logic::plural(*count, "request")
            ),
            emphasis: median_bin == Some(bin),
        })
        .collect();
    chart_card(
        p,
        "latency",
        "Transcription latency",
        &format!(
            "p50 {} · p95 {}",
            logic::format_latency(stats.p50),
            logic::format_latency(stats.p95)
        ),
        charts::bars(p, "latency-bars", bars, CHART_HEIGHT, 2.0),
        charts::axis(
            p,
            "0",
            &format!("max {}", logic::format_latency(stats.max)),
            &format!("p99 {}", logic::format_latency(Some(ceiling))),
        ),
    )
}

fn models(p: Paint, stats: &Stats) -> El {
    let cell = |value: String, heading: bool| {
        text(value)
            .font(p.numeric(p.k.ty.small))
            .color(if heading { p.faint() } else { p.muted() })
            .w(96.0)
            .align(rune::Align::End)
    };
    let line = |key: String, name: &str, cells: [String; 3], heading: bool| {
        row()
            .key(key)
            .min_h(if heading { 32.0 } else { controls::ROW_H })
            .px(ROW_PX)
            .gap(12.0)
            .center_y()
            .child(
                text(name)
                    .font(if heading { p.k.ty.small } else { p.k.ty.ui })
                    .color(if heading { p.faint() } else { p.text() })
                    .truncate()
                    .grow(1.0)
                    .shrink(),
            )
            .children(cells.map(|value| cell(value, heading)))
    };
    let mut rows = vec![line(
        "models-head".to_string(),
        "Model",
        [
            "Recordings".to_string(),
            "Audio".to_string(),
            "p95 latency".to_string(),
        ],
        true,
    )];
    rows.extend(stats.models.iter().enumerate().map(
        |(index, (model, recordings, duration_ms, p95))| {
            line(
                format!("model-{index}"),
                if model.is_empty() { "Unknown" } else { model },
                [
                    logic::format_count(*recordings),
                    logic::format_hours(*duration_ms),
                    logic::format_latency(*p95),
                ],
                false,
            )
        },
    ));
    controls::section(
        p,
        "models",
        "Models",
        None,
        col().children(controls::separated(p, rows)),
    )
}
