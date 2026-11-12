//! Drawing the island: one surface hung from the top edge, square where it
//! meets the edge and round below, with concave fillets that blend it into
//! the edge. Each content layer is laid out at its own state's size and
//! centred in the surface, which clips it while the shape morphs.

use rune::{Color, Cx, El, LineCap, Path, Shape, TextStyle, canvas, el, row, spacer, text};

use super::super::controls::Paint;
use super::Island;
use super::model::{BARS, Model};
use super::motion::{Content, Layer};

/// The overlay surface: room for the largest state and its shadow.
pub(super) const SURFACE: (f32, f32) = (560.0, 132.0);
const PILL_H: f32 = 40.0;
const PAD: f32 = 16.0;
const GAP: f32 = 12.0;
const RADIUS_MAX: f32 = 22.0;
/// The concave fillets where the island leaves the edge.
const FILLET: f32 = 10.0;
const BAR_PITCH: f32 = 5.0;
const BAR_W: f32 = 3.0;
const WAVE_H: f32 = 22.0;
const DOT: f32 = 8.0;
const DOT_BOX: f32 = 18.0;
const BADGE: f32 = 22.0;
const SWEEP_DOTS: usize = 5;
const SWEEP_PITCH: f32 = 6.0;
const SWEEP_PERIOD: f64 = 0.95;
/// The widest a transcript or error line runs before it wraps.
const LINE_MAX: f32 = 340.0;
const LINE_H: f32 = 18.0;
const TALL_PAD_Y: f32 = 12.0;

fn time_style(p: Paint) -> TextStyle {
    p.numeric(p.k.ty.ui_medium)
}

fn label_style(p: Paint) -> TextStyle {
    p.k.ty.ui
}

fn provider_style(p: Paint) -> TextStyle {
    p.k.ty.small
}

fn line_style(p: Paint) -> TextStyle {
    p.k.ty.ui.lh(LINE_H)
}

fn measure(value: &str, style: TextStyle, max_width: Option<f32>) -> (f32, f32) {
    rune::text::measure(value, &[], style, max_width)
}

/// `m:ss`, the way a stopwatch reads.
pub(super) fn clock(milliseconds: i64) -> String {
    let seconds = milliseconds.max(0) / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn wave_width() -> f32 {
    BARS as f32 * BAR_PITCH
}

fn sweep_width() -> f32 {
    SWEEP_DOTS as f32 * SWEEP_PITCH
}

/// The island's size for `content`; `elapsed_ms` is the recording's clock.
pub(super) fn size(content: &Content, p: Paint, elapsed_ms: i64) -> (f32, f32) {
    match content {
        Content::Recording { provider } => {
            let time = measure(&clock(elapsed_ms), time_style(p), None).0;
            let mut width = PAD * 2.0 + DOT_BOX + GAP + wave_width() + GAP + time;
            if let Some(name) = provider {
                width += measure(name, provider_style(p), None).0 + GAP + p.hair + GAP;
            }
            (width.ceil(), PILL_H)
        }
        Content::Transcribing { duration_ms } => {
            let label = measure("Transcribing", label_style(p), None).0;
            let time = measure(&clock(*duration_ms), time_style(p), None).0;
            let width = PAD * 2.0 + sweep_width() + 8.0 + label + 10.0 + time;
            (width.ceil(), PILL_H)
        }
        Content::Pasted { transcript: None } => {
            let label = measure("Pasted", label_style(p), None).0;
            ((PAD * 2.0 + BADGE + 10.0 + label).ceil(), PILL_H)
        }
        Content::Pasted {
            transcript: Some(line),
        }
        | Content::Failed { message: line } => {
            let (width, height) = measure(line, line_style(p), Some(LINE_MAX));
            (
                (PAD * 2.0 + BADGE + GAP + width).ceil(),
                (height + TALL_PAD_Y * 2.0).max(PILL_H).ceil(),
            )
        }
    }
}

/// The island's own surface colour, and the tint an error mixes in.
fn surface(p: Paint, tint: f32) -> Color {
    let base = if p.k.dark { p.k.c.black } else { p.k.c.bg_base };
    base.mix(p.danger(), 0.15 * tint.clamp(0.0, 1.0))
}

pub(super) fn view(island: &Island, cx: &mut Cx<Island>) -> El {
    let p = Paint::new(island.dark, cx.env().scale);
    let motion = &island.motion;
    let (width, height) = (motion.width.x.max(0.0), motion.height.x);
    let root = el()
        .key("osd")
        .size(SURFACE.0, SURFACE.1)
        .instant_changes()
        .role("status")
        .label(status(&island.model));
    if island.first_frame.is_some() {
        return root.child(warm_up(island, p));
    }
    if height < 0.25 {
        return root;
    }
    let left = ((SURFACE.0 - width) / 2.0).round();
    let radius = (height / 2.0).min(RADIUS_MAX);
    let corners = [0.0, 0.0, radius, radius];
    let fill = surface(p, motion.tint.x);
    let elapsed = island.model.elapsed_ms(island.now);
    let mut body = el()
        .key("island")
        .pin(Some(left), Some(0.0), None, None)
        .size(width, height)
        .corners(corners)
        .fill(fill)
        .clip_corners(corners);
    if motion.tint.x > 0.01 {
        body = body.border(p.hair, p.k.c.danger.border.alpha(motion.tint.x));
    } else if !p.k.dark {
        body = body.border(p.hair, p.k.c.border_weak);
    }
    for shadow in &p.k.elevation.floating {
        body = body.box_shadow(*shadow);
    }
    let layers = motion
        .layers
        .iter()
        .map(|layer| place(island, p, layer, (width, height), elapsed));
    root.child(body.children(layers))
        .child(fillets(left, width, height, fill))
}

fn status(model: &Model) -> &'static str {
    match model.mode {
        super::model::Mode::Recording { .. } => "Recording",
        super::model::Mode::Transcribing { .. } => "Transcribing",
        super::model::Mode::Pasted { .. } => "Pasted",
        super::model::Mode::Failed { .. } => "Dictation failed",
        _ => "",
    }
}

/// The concave corners either side of where the island meets the edge.
fn fillets(left: f32, width: f32, height: f32, fill: Color) -> El {
    let size = FILLET.min(height * 0.5);
    let k = 0.552_285 * size;
    let right = left + width;
    let ear_left = Path::new()
        .move_to([left, 0.0])
        .line_to([left, size])
        .cubic_to([left, size - k], [left - size + k, 0.0], [left - size, 0.0])
        .close();
    let ear_right = Path::new()
        .move_to([right, 0.0])
        .line_to([right, size])
        .cubic_to(
            [right, size - k],
            [right + size - k, 0.0],
            [right + size, 0.0],
        )
        .close();
    let shape = Shape::new()
        .fill_path(&ear_left, fill)
        .fill_path(&ear_right, fill);
    canvas(shape)
        .key("fillets")
        .pin(Some(0.0), Some(0.0), None, None)
        .size(SURFACE.0, FILLET)
        .a11y_hidden()
}

/// Every content, blurred and all but transparent, drawn in the frame the
/// island waits on before it drops: the renderer builds its pipelines and
/// glyphs here instead of stalling the first time each state arrives.
fn warm_up(island: &Island, p: Paint) -> El {
    let contents = [
        Content::Recording {
            provider: Some("openai".into()),
        },
        Content::Transcribing { duration_ms: 0 },
        Content::Pasted {
            transcript: Some("The quick brown fox jumps over the lazy dog, 0123456789.".into()),
        },
        Content::Pasted { transcript: None },
        Content::Failed {
            message: "transcription failed: provider timed out".into(),
        },
    ];
    let layers = contents.into_iter().map(|content| {
        let layer = Layer::settled(content);
        let (width, height) = size(&layer.content, p, 0);
        content_of(island, p, &layer)
            .size(width, height)
            .pin(Some(0.0), Some(0.0), None, None)
            .blur(2.0)
    });
    el().key("warm-up")
        .size(SURFACE.0, PILL_H * 2.0)
        .pin(Some(0.0), Some(SURFACE.1 - PILL_H * 2.0), None, None)
        .fill(surface(p, 1.0))
        .opacity(0.01)
        .a11y_hidden()
        .children(layers)
}

/// A content layer at its own size, centred in the island, faded and slid
/// by how far it has arrived or left.
fn place(island: &Island, p: Paint, layer: &Layer, island_size: (f32, f32), elapsed: i64) -> El {
    let (width, height) = size(&layer.content, p, elapsed);
    let presence = layer.presence.x;
    let (opacity, lift, blur) = if layer.leaving() {
        let left = presence.clamp(0.0, 1.0);
        (left, -(1.0 - left) * 5.0, (1.0 - left) * 4.0)
    } else {
        (presence.clamp(0.0, 1.0), (1.0 - presence) * 10.0, 0.0)
    };
    let mut placed = content_of(island, p, layer)
        .size(width, height)
        .pin(
            Some(((island_size.0 - width) / 2.0).round()),
            Some((island_size.1 - height) / 2.0 + lift),
            None,
            None,
        )
        .opacity(opacity);
    if blur > 0.05 {
        placed = placed.blur(blur);
    }
    placed
}

fn content_of(island: &Island, p: Paint, layer: &Layer) -> El {
    let now = island.now;
    let since = (now - layer.born).max(0.0);
    match &layer.content {
        Content::Recording { provider } => {
            let provider = provider.as_ref().map(|name| {
                row()
                    .key("provider")
                    .gap(GAP)
                    .center_y()
                    .child(text(name).font(provider_style(p)).color(p.faint()))
                    .child(el().w(p.hair).h(14.0).fill(p.k.c.border_base))
            });
            row()
                .key("recording")
                .px(PAD)
                .gap(GAP)
                .center_y()
                .children(provider)
                .child(record_dot(p, now))
                .child(waveform(
                    p,
                    &island.model.levels.bars,
                    island.model.recording(),
                ))
                .child(spacer())
                .child(
                    text(clock(island.model.elapsed_ms(now)))
                        .font(time_style(p))
                        .color(p.strong()),
                )
        }
        Content::Transcribing { duration_ms } => row()
            .key("transcribing")
            .px(PAD)
            .gap(8.0)
            .center_y()
            .child(sheen(p, since, size(&layer.content, p, 0)))
            .child(sweep(p, since))
            .child(text("Transcribing").font(label_style(p)).color(p.text()))
            .child(spacer())
            .child(
                text(clock(*duration_ms))
                    .font(time_style(p))
                    .color(p.muted()),
            ),
        Content::Pasted { transcript: None } => row()
            .key("pasted")
            .px(PAD)
            .gap(10.0)
            .center_y()
            .child(check_badge(p, since))
            .child(text("Pasted").font(label_style(p)).color(p.strong())),
        Content::Pasted {
            transcript: Some(line),
        } => tall(p, "pasted", check_badge(p, since), line),
        Content::Failed { message } => tall(p, "failed", alert_badge(p), message),
    }
}

/// A badge beside a line or two of text.
fn tall(p: Paint, key: &str, badge: El, line: &str) -> El {
    let (width, _) = measure(line, line_style(p), Some(LINE_MAX));
    row()
        .key(key)
        .px(PAD)
        .gap(GAP)
        .center_y()
        .child(badge)
        .child(
            text(line)
                .font(line_style(p))
                .color(p.strong())
                .max_lines(2)
                .w(width),
        )
}

/// The record light: a steady dot with a ring that breathes out from it.
fn record_dot(p: Paint, now: f64) -> El {
    let phase = (now / 1.4).fract() as f32;
    let ring = DOT + (DOT_BOX - DOT) * phase;
    let centre = |size: f32| Some((DOT_BOX - size) / 2.0);
    let glow = 0.82 + 0.18 * (now as f32 * std::f32::consts::TAU / 1.4).cos();
    el().key("dot")
        .size(DOT_BOX, DOT_BOX)
        .a11y_hidden()
        .child(
            el().size(ring, ring)
                .radius(ring / 2.0)
                .fill(p.danger().alpha(0.4 * (1.0 - phase) * (1.0 - phase)))
                .pin(centre(ring), centre(ring), None, None),
        )
        .child(
            el().size(DOT, DOT)
                .radius(DOT / 2.0)
                .fill(p.danger().alpha(glow))
                .pin(centre(DOT), centre(DOT), None, None),
        )
}

/// Mirrored level bars, tallest at the centre. They lie flat as dots once
/// the recording ends.
fn waveform(p: Paint, bars: &[f32; BARS], live: bool) -> El {
    let middle = WAVE_H / 2.0;
    let ink = if live { p.strong() } else { p.muted() };
    let mut shape = Shape::new();
    for (index, level) in bars.iter().enumerate() {
        let x = index as f32 * BAR_PITCH + BAR_PITCH / 2.0;
        let half = level.clamp(0.0, 1.0) * (WAVE_H - BAR_W) / 2.0;
        shape = shape.line(
            [x, middle - half],
            [x, middle + half],
            BAR_W,
            LineCap::Round,
            ink.alpha(0.5 + 0.5 * level.clamp(0.0, 1.0)),
        );
    }
    canvas(shape)
        .key("wave")
        .size(wave_width(), WAVE_H)
        .a11y_hidden()
}

/// Where the transcribing crest is, from the first dot (0) to past the
/// last (1).
fn crest(since: f64) -> f32 {
    (since / SWEEP_PERIOD).fract() as f32
}

/// A row of dots a bright crest runs along while the server works.
fn sweep(p: Paint, since: f64) -> El {
    let middle = WAVE_H / 2.0;
    let crest = crest(since) * (SWEEP_DOTS as f32 + 2.0) - 1.0;
    let mut shape = Shape::new();
    for index in 0..SWEEP_DOTS {
        let x = index as f32 * SWEEP_PITCH + SWEEP_PITCH / 2.0;
        let distance = index as f32 - crest;
        let lit = (-distance * distance / 1.6).exp();
        let half = lit * 4.0;
        shape = shape.line(
            [x, middle - half],
            [x, middle + half],
            BAR_W,
            LineCap::Round,
            p.strong().alpha(0.28 + 0.72 * lit),
        );
    }
    canvas(shape)
        .key("sweep")
        .size(sweep_width(), WAVE_H)
        .a11y_hidden()
}

/// A soft band of light that follows the crest on across the label.
fn sheen(p: Paint, since: f64, (width, height): (f32, f32)) -> El {
    const BAND: f32 = 72.0;
    let x = crest(since) * (width + BAND) - BAND;
    let light = p.strong();
    let strength = if p.k.dark { 0.07 } else { 0.035 };
    el().key("sheen")
        .size(BAND, height)
        .pin(Some(x), Some(0.0), None, None)
        .linear_gradient(
            std::f32::consts::FRAC_PI_2,
            [
                (0.0, light.alpha(0.0)),
                (0.5, light.alpha(strength)),
                (1.0, light.alpha(0.0)),
            ],
        )
        .a11y_hidden()
}

fn badge(key: &str, fill: Color, mark: Shape) -> El {
    el().key(key)
        .size(BADGE, BADGE)
        .radius(BADGE / 2.0)
        .fill(fill)
        .a11y_hidden()
        .child(canvas(mark).size(BADGE, BADGE))
}

/// A check that draws itself on as the paste lands.
fn check_badge(p: Paint, since: f64) -> El {
    let drawn = ((since as f32 - 0.04) / 0.3).clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - drawn).powi(3);
    let ink = p.success();
    let mark = Shape::new()
        .stroke_path(
            &Path::new()
                .move_to([6.5, 11.5])
                .line_to([9.8, 14.6])
                .line_to([15.5, 7.8]),
            2.0,
            LineCap::Round,
            ink,
        )
        .drawn(eased);
    badge("check", ink.alpha(0.18), mark)
}

fn alert_badge(p: Paint) -> El {
    let ink = p.danger();
    let mark = Shape::new()
        .line([11.0, 6.5], [11.0, 12.2], 2.0, LineCap::Round, ink)
        .line([11.0, 15.6], [11.0, 15.6], 2.4, LineCap::Round, ink);
    badge("alert", ink.alpha(0.2), mark)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_reads_minutes_and_padded_seconds() {
        for (milliseconds, expected) in
            [(0, "0:00"), (7_900, "0:07"), (61_000, "1:01"), (-5, "0:00")]
        {
            assert_eq!(clock(milliseconds), expected);
        }
    }
}
