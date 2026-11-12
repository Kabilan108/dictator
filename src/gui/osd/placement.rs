//! Where the island sits, as data: the edge it hangs from, how far from it,
//! its corner and fillet shape and which way it grows. Motion and view read
//! these and are otherwise the same for every placement.

use rune::ScreenEdge;

/// The tab an attached island shows as it comes out of its edge.
const TAB: (f32, f32) = (64.0, 12.0);
/// The dot a floating island appears as and closes back into.
const DOT: f32 = 14.0;
/// How long the tab shows before the island springs open; a floating dot
/// holds a little longer, so it reads as a dot before it opens.
const TAB_HOLD: f64 = 0.18;
const DOT_HOLD: f64 = 0.28;
/// How far a floating island sits from the bottom edge.
const FLOAT_GAP: f32 = 28.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    /// Hung from the top edge.
    Top,
    /// Standing on the bottom edge.
    #[default]
    Bottom,
    /// A capsule above the bottom edge, clear of it.
    Floating,
}

impl Placement {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            "floating" => Some(Self::Floating),
            _ => None,
        }
    }

    /// The screen edge the overlay surface is anchored to.
    pub(super) fn edge(self) -> ScreenEdge {
        match self {
            Self::Top => ScreenEdge::Top,
            Self::Bottom | Self::Floating => ScreenEdge::Bottom,
        }
    }

    /// Whether the island is part of the edge: square where it meets it,
    /// blended in by concave fillets, and grown out of it rather than faded.
    pub(super) fn attached(self) -> bool {
        self != Self::Floating
    }

    /// The gap between the screen edge and the island, inside the surface.
    pub(super) fn gap(self) -> f32 {
        if self.attached() { 0.0 } else { FLOAT_GAP }
    }

    /// The direction away from the edge on screen: 1 down, -1 up.
    pub(super) fn away(self) -> f32 {
        match self.edge() {
            ScreenEdge::Top => 1.0,
            ScreenEdge::Bottom => -1.0,
        }
    }

    /// The island's top within a surface `surface_h` tall.
    pub(super) fn top(self, height: f32, surface_h: f32) -> f32 {
        match self.edge() {
            ScreenEdge::Top => self.gap(),
            ScreenEdge::Bottom => surface_h - self.gap() - height,
        }
    }

    /// Corner radii, `[top_left, top_right, bottom_right, bottom_left]`:
    /// square against the edge, `radius` everywhere else.
    pub(super) fn corners(self, radius: f32) -> [f32; 4] {
        match self {
            Self::Top => [0.0, 0.0, radius, radius],
            Self::Bottom => [radius, radius, 0.0, 0.0],
            Self::Floating => [radius; 4],
        }
    }

    /// The size the island starts from and goes back to: no height inside
    /// the edge, or a dot.
    pub(super) fn seed(self) -> (f32, f32) {
        if self.attached() {
            (TAB.0, 0.0)
        } else {
            (DOT, DOT)
        }
    }

    /// The size it holds for a moment before it springs open.
    pub(super) fn tab(self) -> (f32, f32) {
        if self.attached() { TAB } else { (DOT, DOT) }
    }

    /// How long it holds that size.
    pub(super) fn tab_hold(self) -> f64 {
        if self.attached() { TAB_HOLD } else { DOT_HOLD }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SURFACE_H: f32 = 132.0;

    #[test]
    fn placements_parse_by_name() {
        assert_eq!(Placement::parse("top"), Some(Placement::Top));
        assert_eq!(Placement::parse("bottom"), Some(Placement::Bottom));
        assert_eq!(Placement::parse("floating"), Some(Placement::Floating));
        assert_eq!(Placement::parse("left"), None);
        assert_eq!(Placement::default(), Placement::Bottom);
    }

    #[test]
    fn top_hangs_from_the_top_edge_and_grows_down() {
        let top = Placement::Top;
        assert_eq!(top.edge(), ScreenEdge::Top);
        assert!(top.attached());
        assert_eq!(top.top(40.0, SURFACE_H), 0.0);
        assert_eq!(top.away(), 1.0);
        assert_eq!(top.corners(20.0), [0.0, 0.0, 20.0, 20.0]);
        assert_eq!(top.seed().1, 0.0);
    }

    #[test]
    fn bottom_mirrors_top_against_the_bottom_edge() {
        let bottom = Placement::Bottom;
        assert_eq!(bottom.edge(), ScreenEdge::Bottom);
        assert!(bottom.attached());
        assert_eq!(bottom.top(40.0, SURFACE_H), SURFACE_H - 40.0);
        assert_eq!(bottom.top(64.0, SURFACE_H), SURFACE_H - 64.0, "grows up");
        assert_eq!(bottom.away(), -Placement::Top.away());
        assert_eq!(bottom.corners(20.0), [20.0, 20.0, 0.0, 0.0]);
        assert_eq!(bottom.seed(), Placement::Top.seed());
        assert_eq!(bottom.tab(), Placement::Top.tab());
    }

    #[test]
    fn floating_is_a_round_capsule_clear_of_the_bottom_edge() {
        let floating = Placement::Floating;
        assert_eq!(floating.edge(), ScreenEdge::Bottom);
        assert!(!floating.attached(), "no fillets");
        assert_eq!(floating.top(40.0, SURFACE_H), SURFACE_H - FLOAT_GAP - 40.0);
        assert_eq!(floating.corners(20.0), [20.0; 4]);
        let (width, height) = floating.seed();
        assert!(width == height && height > 0.0, "starts as a dot");
    }
}
