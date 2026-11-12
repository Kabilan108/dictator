//! Drawing the island: floating, a capsule round all over; attached, one
//! surface on its edge, square where it meets the edge and round elsewhere,
//! with concave fillets that blend it into the edge. Each content layer is
//! laid out at its own state's size and centred in the surface, which clips
//! it while the shape morphs. The dot scope is drawn once, over the layers,
//! wherever the motion has slid it.

use rune::{Color, Cx, El, LineCap, Path, ScreenEdge, Shape, TextStyle, canvas, el, row, text};

use super::super::controls::Paint;
use super::Island;
use super::model::{Mode, Model};
use super::motion::{Content, Fit, Layer};
use super::scope::{COLUMNS, Dot, ROWS};

/// The overlay surface: room for the largest state, a floating island's gap
/// and its shadow.
pub(super) const SURFACE: (f32, f32) = (560.0, 132.0);
const PILL_H: f32 = 40.0;
const PAD: f32 = 16.0;
const GAP: f32 = 12.0;
const RADIUS_MAX: f32 = 22.0;
/// The concave fillets where the island leaves the edge.
const FILLET: f32 = 10.0;
const DOT: f32 = 8.0;
const DOT_BOX: f32 = 18.0;
const BADGE: f32 = 22.0;
/// The dot scope: dot spacing and size, and the bloom drawn under it.
const PITCH: f32 = 6.0;
const SCOPE_DOT: f32 = 3.2;
const GLOW_DOT: f32 = 7.0;
const GLOW_BLUR: f32 = 2.2;
const SCOPE_W: f32 = COLUMNS as f32 * PITCH;
const SCOPE_H: f32 = PILL_H;
/// Room either side of the scope for the newest column's bloom.
const BLOOM_ROOM: f32 = 6.0;
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

/// How `content` lays out; `elapsed_ms` is the recording's clock.
pub(super) fn fit(content: &Content, p: Paint, elapsed_ms: i64) -> Fit {
    match content {
        Content::Recording { provider } => {
            let time = measure(&clock(elapsed_ms), time_style(p), None).0;
            let lead = provider.as_ref().map_or(0.0, |name| {
                measure(name, provider_style(p), None).0 + GAP + p.hair + GAP
            });
            let scope = PAD + lead + DOT_BOX + GAP;
            Fit {
                size: ((scope + SCOPE_W + GAP + time + PAD).ceil(), PILL_H),
                scope: Some(scope),
            }
        }
        Content::Transcribing { duration_ms } => {
            let label = measure("Transcribing", label_style(p), None).0;
            let time = measure(&clock(*duration_ms), time_style(p), None).0;
            let width = PAD * 2.0 + SCOPE_W + GAP + label + GAP + time;
            Fit {
                size: (width.ceil(), PILL_H),
                scope: Some(PAD),
            }
        }
        Content::Pasted { transcript: None } => {
            let label = measure("Pasted", label_style(p), None).0;
            Fit {
                size: ((PAD * 2.0 + BADGE + 10.0 + label).ceil(), PILL_H),
                scope: None,
            }
        }
        Content::Pasted {
            transcript: Some(line),
        }
        | Content::Failed { message: line } => {
            let (width, height) = measure(line, line_style(p), Some(LINE_MAX));
            Fit {
                size: (
                    (PAD * 2.0 + BADGE + GAP + width).ceil(),
                    (height + TALL_PAD_Y * 2.0).max(PILL_H).ceil(),
                ),
                scope: None,
            }
        }
    }
}

/// The island's own surface colour, and the tint an error mixes in.
fn surface(p: Paint, tint: f32) -> Color {
    let base = if p.k.dark { p.k.c.black } else { p.k.c.bg_base };
    base.mix(p.danger(), 0.15 * tint.clamp(0.0, 1.0))
}

pub(super) fn view(island: &Island, cx: &mut Cx<Island>) -> El {
    let dark = island.dark.unwrap_or_else(|| cx.system_dark());
    let p = Paint::new(dark, cx.env().scale);
    let placement = island.placement;
    let motion = &island.motion;
    let height = motion.height.x;
    let width = if placement.attached() {
        motion.width.x.max(0.0)
    } else {
        motion.width.x.max(height)
    };
    let root = el()
        .key("osd")
        .size(SURFACE.0, SURFACE.1)
        .instant_changes()
        .role("status")
        .label(status(&island.model));
    if island.first_frame.is_some() {
        return root.child(warm_up(island, p));
    }
    if height < 0.25 || motion.fade.x < 0.005 {
        return root;
    }
    let left = ((SURFACE.0 - width) / 2.0).round();
    let top = placement.top(height, SURFACE.1) + placement.away() * motion.rise.x;
    let corners = placement.corners((height / 2.0).min(RADIUS_MAX));
    let fill = surface(p, motion.tint.x);
    let elapsed = island.model.elapsed_ms(island.now);
    let mut body = el()
        .key("island")
        .pin(Some(left), Some(top), None, None)
        .size(width, height)
        .corners(corners)
        .fill(fill)
        .clip_corners(corners);
    if motion.fade.x < 1.0 {
        body = body.opacity(motion.fade.x.clamp(0.0, 1.0));
    }
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
    let mut body = body.children(layers);
    if motion.scope.x > 0.005 {
        let (opacity, lift, blur) = arrival(motion.scope.x, motion.scope_leaving());
        let mut scope = scope(island, p)
            .pin(
                Some((width / 2.0 + motion.scope_x.x - BLOOM_ROOM).round()),
                Some((height - SCOPE_H) / 2.0 + lift * placement.away()),
                None,
                None,
            )
            .opacity(opacity);
        if blur > 0.05 {
            scope = scope.blur(blur);
        }
        body = body.child(scope);
    }
    let root = root.child(body);
    if placement.attached() {
        root.child(fillets(placement.edge(), left, width, height, fill))
    } else {
        root
    }
}

fn status(model: &Model) -> &'static str {
    match model.mode {
        Mode::Recording { .. } => "Recording",
        Mode::Transcribing { .. } => "Transcribing",
        Mode::Pasted { .. } => "Pasted",
        Mode::Failed { .. } => "Dictation failed",
        _ => "",
    }
}

/// The concave corners either side of where the island meets its edge.
fn fillets(edge: ScreenEdge, left: f32, width: f32, height: f32, fill: Color) -> El {
    let size = FILLET.min(height * 0.5);
    let k = 0.552_285 * size;
    let right = left + width;
    let (top, y): (f32, fn(f32) -> f32) = match edge {
        ScreenEdge::Top => (0.0, |y| y),
        ScreenEdge::Bottom => (SURFACE.1 - FILLET, |y| FILLET - y),
    };
    let ear = |side: f32, x: f32| {
        Path::new()
            .move_to([x, y(0.0)])
            .line_to([x, y(size)])
            .cubic_to(
                [x, y(size - k)],
                [x + side * (size - k), y(0.0)],
                [x + side * size, y(0.0)],
            )
            .close()
    };
    let shape = Shape::new()
        .fill_path(&ear(-1.0, left), fill)
        .fill_path(&ear(1.0, right), fill);
    canvas(shape)
        .key("fillets")
        .pin(Some(0.0), Some(top), None, None)
        .size(SURFACE.0, FILLET)
        .a11y_hidden()
}

/// Every content, blurred and all but transparent, drawn in the frame the
/// island waits on before it shows: the renderer builds its pipelines and
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
            message: "Transcription failed".into(),
        },
    ];
    let layers = contents.into_iter().map(|content| {
        let layer = Layer::settled(content);
        let (width, height) = fit(&layer.content, p, 0).size;
        content_of(island, p, &layer)
            .size(width, height)
            .pin(Some(0.0), Some(0.0), None, None)
            .blur(2.0)
    });
    let far_side = if island.placement.away() > 0.0 {
        SURFACE.1 - PILL_H * 2.0
    } else {
        0.0
    };
    el().key("warm-up")
        .size(SURFACE.0, PILL_H * 2.0)
        .pin(Some(0.0), Some(far_side), None, None)
        .fill(surface(p, 1.0))
        .opacity(0.01)
        .a11y_hidden()
        .children(layers)
        .child(scope(island, p).blur(1.0))
}

/// How far along arriving or leaving something is: its opacity, how far it
/// has slid (towards the edge, in points) and its blur.
fn arrival(presence: f32, leaving: bool) -> (f32, f32, f32) {
    if leaving {
        let left = presence.clamp(0.0, 1.0);
        (left, -(1.0 - left) * 5.0, (1.0 - left) * 4.0)
    } else {
        (presence.clamp(0.0, 1.0), (1.0 - presence) * 10.0, 0.0)
    }
}

/// A content layer at its own size, centred in the island, faded and slid
/// by how far it has arrived or left: in from away from the edge, out
/// towards it.
fn place(island: &Island, p: Paint, layer: &Layer, island_size: (f32, f32), elapsed: i64) -> El {
    let (width, height) = fit(&layer.content, p, elapsed).size;
    let (opacity, lift, blur) = arrival(layer.presence.x, layer.leaving());
    let mut placed = content_of(island, p, layer)
        .size(width, height)
        .pin(
            Some(((island_size.0 - width) / 2.0).round()),
            Some((island_size.1 - height) / 2.0 + lift * island.placement.away()),
            None,
            None,
        )
        .opacity(opacity);
    if blur > 0.05 {
        placed = placed.blur(blur);
    }
    placed
}

/// Where the dot scope sits in a layout; the scope itself is drawn on top.
fn scope_slot() -> El {
    el().w(SCOPE_W).h(SCOPE_H).a11y_hidden()
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
                .child(scope_slot())
                .child(
                    text(clock(island.model.elapsed_ms(now)))
                        .font(time_style(p))
                        .color(p.strong()),
                )
        }
        Content::Transcribing { duration_ms } => row()
            .key("transcribing")
            .px(PAD)
            .gap(GAP)
            .center_y()
            .child(scope_slot())
            .child(text("Transcribing").font(label_style(p)).color(p.text()))
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
        } => tall(p, "pasted-line", check_badge(p, since), line),
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

struct Inks {
    rest: Color,
    ink: Color,
    /// Bloom reads as light on a dark island and as a smudge on a light one.
    bloom: f32,
}

impl Inks {
    fn new(p: Paint) -> Self {
        Self {
            rest: p.faint().alpha(if p.k.dark { 0.3 } else { 0.36 }),
            ink: p.strong(),
            bloom: if p.k.dark { 0.6 } else { 0.3 },
        }
    }
}

/// The centre of the dot at `column` and `row` within the scope's box.
fn dot_centre(column: usize, row: usize) -> [f32; 2] {
    [
        BLOOM_ROOM + (column as f32 + 0.5) * PITCH,
        SCOPE_H / 2.0 + (row as f32 - (ROWS / 2) as f32) * PITCH,
    ]
}

fn dot_at(shape: Shape, at: [f32; 2], diameter: f32, color: Color) -> Shape {
    let half = diameter * 0.01;
    shape.line(
        [at[0] - half, at[1]],
        [at[0] + half, at[1]],
        diameter,
        LineCap::Round,
        color,
    )
}

/// The dot scope: every dot at rest, lit towards the ink by its level, over
/// a blurred bloom for the brightest.
fn scope(island: &Island, p: Paint) -> El {
    let dots: [[Dot; ROWS]; COLUMNS] = island.scope.dots(island.now);
    let inks = Inks::new(p);
    let (mut matrix, mut bloom) = (Shape::new(), Shape::new());
    for (column, cells) in dots.iter().enumerate() {
        for (row, dot) in cells.iter().enumerate() {
            let at = dot_centre(column, row);
            matrix = dot_at(matrix, at, SCOPE_DOT, inks.rest.mix(inks.ink, dot.light));
            if dot.glow > 0.02 {
                bloom = dot_at(bloom, at, GLOW_DOT, inks.ink.alpha(dot.glow * inks.bloom));
            }
        }
    }
    let size = (SCOPE_W + BLOOM_ROOM * 2.0, SCOPE_H);
    el().key("scope")
        .size(size.0, size.1)
        .a11y_hidden()
        .child(
            canvas(bloom)
                .key("bloom")
                .pin(Some(0.0), Some(0.0), None, None)
                .size(size.0, size.1)
                .blur(GLOW_BLUR),
        )
        .child(
            canvas(matrix)
                .key("matrix")
                .pin(Some(0.0), Some(0.0), None, None)
                .size(size.0, size.1),
        )
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
