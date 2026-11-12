//! Shared pieces the panes are built from. Colours, type, radii and elevation
//! come from the kit's tokens; the numbers here are layout metrics.

use rune::{Color, El, Icon, ScrollSpec, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, ScrollBar,
    ScrollBarStyle,
};

use super::logic::ErrorSummary;
use super::{Ui, icons};

pub(super) const GUTTER: f32 = 28.0;
pub(super) const SECTION_GAP: f32 = 24.0;
pub(super) const ROW_PX: f32 = 16.0;
pub(super) const ROW_H: f32 = 44.0;
const LABEL_GAP: f32 = 8.0;

/// Everything a view needs to paint itself.
#[derive(Clone, Copy)]
pub(super) struct Paint {
    pub(super) k: &'static Kit,
    /// One device pixel in points.
    pub(super) hair: f32,
}

impl Paint {
    pub(super) fn new(dark: bool, scale: f32) -> Self {
        Self {
            k: rune_theme_opencode::kit(dark),
            hair: 1.0 / scale.max(1.0),
        }
    }

    pub(super) fn text(self) -> Color {
        self.k.c.text_base
    }

    pub(super) fn strong(self) -> Color {
        self.k.c.text_strong
    }

    pub(super) fn muted(self) -> Color {
        self.k.c.text_muted
    }

    pub(super) fn faint(self) -> Color {
        self.k.c.text_faint
    }

    pub(super) fn danger(self) -> Color {
        self.k.c.danger.fg
    }

    pub(super) fn success(self) -> Color {
        self.k.c.success.fg
    }

    pub(super) fn warning(self) -> Color {
        self.k.c.warning.fg
    }

    pub(super) fn accent(self) -> Color {
        self.k.c.text_accent
    }

    /// Tabular figures, so numbers that tick do not shift their neighbours.
    pub(super) fn numeric(self, style: TextStyle) -> TextStyle {
        style.features("'tnum' 1")
    }
}

/// The muted label above a card.
pub(super) fn section_label(p: Paint, title: &str) -> El {
    text(title)
        .role("heading")
        .level(2)
        .font(p.k.ty.ui_medium.lh(16.0))
        .color(p.muted())
}

/// A raised surface with the kit's card elevation.
pub(super) fn raised(p: Paint, key: &str) -> El {
    let mut surface = col()
        .key(key)
        .fill(p.k.c.surface_raised_base)
        .radius(p.k.radius.lg);
    for shadow in &p.k.elevation.raised {
        surface = surface.box_shadow(*shadow);
    }
    surface
}

/// A labelled card. `accessory` sits on the label's line, ending where the
/// card does.
pub(super) fn section(p: Paint, key: &str, title: &str, accessory: Option<El>, body: El) -> El {
    let header = row()
        .key("header")
        .min_h(20.0)
        .mx(ROW_PX)
        .gap(8.0)
        .center_y()
        .child(section_label(p, title).grow(1.0))
        .children(accessory);
    col()
        .key(key)
        .gap(LABEL_GAP)
        .child(header)
        .child(raised(p, "card").child(body))
}

pub(super) fn separator(p: Paint) -> El {
    el().h(p.hair)
        .mx(ROW_PX)
        .fill(p.k.c.border_muted)
        .a11y_hidden()
}

/// Rows with a hairline between each pair.
pub(super) fn separated(p: Paint, rows: Vec<El>) -> Vec<El> {
    let mut children = Vec::with_capacity(rows.len() * 2);
    for (index, item) in rows.into_iter().enumerate() {
        if index > 0 {
            children.push(separator(p).key(format!("sep{index}")));
        }
        children.push(item);
    }
    children
}

/// A setting: title and optional description on the left, a control or value
/// on the right.
pub(super) fn setting_row(
    p: Paint,
    key: &str,
    title: &str,
    description: Option<&str>,
    control: El,
) -> El {
    let copy = col()
        .key("copy")
        .grow(1.0)
        .shrink()
        .min_w(80.0)
        .gap(2.0)
        .child(text(title).font(p.k.ty.ui).color(p.text()))
        .children(description.map(|copy| {
            text(copy)
                .key("description")
                .font(p.k.ty.small.lh(16.0))
                .color(p.muted())
                .wrap()
        }));
    row()
        .key(key)
        .min_h(if description.is_some() { 58.0 } else { ROW_H })
        .px(ROW_PX)
        .py(8.0)
        .gap(16.0)
        .center_y()
        .child(copy)
        .child(control)
}

/// A read-only value at the end of a [`setting_row`].
pub(super) fn value_text(p: Paint, value: &str) -> El {
    text(value)
        .font(p.numeric(p.k.ty.ui))
        .color(p.muted())
        .selectable(true)
        .selection_color(p.k.selection())
}

pub(super) fn button(
    p: Paint,
    key: &str,
    label: &str,
    variant: ButtonVariant,
    on_press: impl Fn(&mut Ui, &mut Tx) + 'static,
) -> El {
    Button::new(label)
        .variant(variant)
        .on_press_tx(on_press)
        .build(p.k)
        .key(key)
}

pub(super) fn small_button(
    p: Paint,
    key: &str,
    label: &str,
    variant: ButtonVariant,
    on_press: impl Fn(&mut Ui, &mut Tx) + 'static,
) -> El {
    Button::new(label)
        .variant(variant)
        .control_size(ControlSize::Small)
        .on_press_tx(on_press)
        .build(p.k)
        .key(key)
}

/// An icon-only action; `label` names it for assistive technology and the
/// tooltip.
pub(super) fn icon_button(
    p: Paint,
    key: &str,
    glyph: &'static Icon,
    label: &str,
    on_press: impl Fn(&mut Ui, &mut Tx) + 'static,
) -> El {
    IconButton::new(glyph)
        .variant(IconButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .label(label)
        .help(label)
        .on_press_tx(on_press)
        .build(p.k)
        .key(key)
}

pub(super) fn disabled_icon_button(p: Paint, key: &str, glyph: &'static Icon, label: &str) -> El {
    IconButton::new(glyph)
        .variant(IconButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .label(label)
        .disabled(true)
        .build(p.k)
        .key(key)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Neutral,
    Good,
    Warn,
    Bad,
    Accent,
}

impl Tone {
    pub(super) fn color(self, p: Paint) -> Color {
        match self {
            Tone::Neutral => p.faint(),
            Tone::Good => p.success(),
            Tone::Warn => p.warning(),
            Tone::Bad => p.danger(),
            Tone::Accent => p.accent(),
        }
    }
}

/// A state: a small dot and a word. Never clickable.
pub(super) fn status_dot(p: Paint, tone: Tone) -> El {
    el().size(7.0, 7.0)
        .radius(3.5)
        .fill(tone.color(p))
        .a11y_hidden()
}

pub(super) fn tone_tag(p: Paint, label: &str, tone: Tone) -> El {
    row()
        .gap(6.0)
        .center_y()
        .child(status_dot(p, tone))
        .child(text(label).font(p.k.ty.small).color(p.muted()))
}

/// A short message under a control.
pub(super) fn note(p: Paint, key: &str, message: &str, tone: Tone) -> El {
    let color = if tone == Tone::Neutral {
        p.muted()
    } else {
        tone.color(p)
    };
    text(message)
        .key(key)
        .font(p.k.ty.small.lh(16.0))
        .color(color)
        .wrap_anywhere()
}

/// A failure: what went wrong in a few words, the provider's own message
/// under it, and what can be done about it.
pub(super) fn error_callout(p: Paint, key: &str, summary: &ErrorSummary, actions: Vec<El>) -> El {
    let detail = (!summary.detail.is_empty()).then(|| {
        text(summary.detail.clone())
            .key("detail")
            .font(p.k.ty.small.lh(17.0))
            .color(p.muted())
            .wrap_anywhere()
            .max_lines(3)
    });
    row()
        .key(key)
        .px(14.0)
        .py(12.0)
        .gap(12.0)
        .center_y()
        .radius(p.k.radius.md)
        .fill(p.k.c.danger.bg)
        .border(p.hair, p.k.c.danger.border)
        .child(
            icon(&icons::ALERT, 16.0)
                .color(p.danger())
                .self_start()
                .mt(1.0),
        )
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(3.0)
                .child(
                    text(summary.title.clone())
                        .key("title")
                        .font(p.k.ty.ui_medium)
                        .color(p.strong())
                        .wrap(),
                )
                .children(detail),
        )
        .children((!actions.is_empty()).then(|| row().gap(6.0).center_y().children(actions)))
}

/// A full-width notice at the top of a pane.
pub(super) fn banner(p: Paint, key: &str, message: &str, tone: Tone, action: Option<El>) -> El {
    let (bg, border) = match tone {
        Tone::Bad => (p.k.c.danger.bg, p.k.c.danger.border),
        Tone::Warn => (p.k.c.warning.bg, p.k.c.warning.border),
        Tone::Good => (p.k.c.success.bg, p.k.c.success.border),
        _ => (p.k.c.info.bg, p.k.c.info.border),
    };
    row()
        .key(key)
        .px(14.0)
        .py(10.0)
        .gap(12.0)
        .center_y()
        .radius(p.k.radius.md)
        .fill(bg)
        .border(p.hair, border)
        .child(
            text(message)
                .font(p.k.ty.ui.lh(18.0))
                .color(p.text())
                .wrap()
                .grow(1.0)
                .shrink(),
        )
        .children(action)
}

/// A quiet centred message for a pane with nothing to show.
pub(super) fn empty_state(
    p: Paint,
    key: &str,
    glyph: &'static Icon,
    title: &str,
    hint: &str,
) -> El {
    col()
        .key(key)
        .grow(1.0)
        .center()
        .gap(6.0)
        .padding(24.0)
        .child(icon(glyph, 22.0).color(p.faint()))
        .child(text(title).font(p.k.ty.ui_medium).color(p.muted()))
        .children((!hint.is_empty()).then(|| text(hint).font(p.k.ty.small).color(p.faint())))
}

/// A vertical scroll viewport that fills what its parent leaves, with the
/// kit's scroll bar over its right edge and a soft fade where content hides.
pub(super) fn scroll_fill(p: Paint, key: &str, content: El) -> El {
    col()
        .key(key)
        .grow(1.0)
        .shrink()
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_edge_fade(12.0)
        .child(content)
        .scroll_bar_styled(scroll_style(p))
        .grow(1.0)
        .shrink()
        .min_h(0.0)
}

fn scroll_style(p: Paint) -> ScrollBarStyle {
    ScrollBarStyle::from_kit(p.k)
        .thumb(p.k.c.text_faint)
        .thumb_hover(p.k.c.text_muted)
}
